use std::borrow::Cow;
use std::cell::Cell;
use std::cell::Ref;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;

use html5ever::Attribute;
use html5ever::LocalName;
use html5ever::Namespace;
use html5ever::QualName;
use html5ever::interface::NodeOrText;
use html5ever::interface::TreeSink;
use html5ever::local_name;
use html5ever::tendril::StrTendril;
use html5ever::tendril::TendrilSink;
use html5ever::tendril::stream::Utf8LossyDecoder;
use taffy::Style;
use taffy::TaffyError;
use taffy::TaffyTree;

#[derive(Debug, thiserror::Error)]
pub enum TreeBuilderError {
	#[error(transparent)]
	Io(#[from] io::Error),
	#[error(transparent)]
	Taffy(#[from] TaffyError),
	#[error("Sibling has no parent")]
	SiblingHasNoParent,
}

#[derive(Debug)]
pub struct Element {
	pub name: QualName,
	pub attrs: BTreeMap<(Namespace, LocalName), String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TextStyle {
	#[default]
	Body,
	H1,
	H2,
}

#[derive(Debug)]
pub struct Text {
	pub style: TextStyle,
	pub t: StrTendril,
}

#[derive(Debug)]
pub(crate) enum Node {
	Element(Element),
	Text(Text),
}

impl Node {
	fn get_style(&self) -> TextStyle {
		match self {
			Node::Element(Element { name, .. }) => match name.local {
				local_name!("h1") => TextStyle::H1,
				local_name!("h2") => TextStyle::H2,
				_ => TextStyle::default(),
			},
			Node::Text(Text { style: s, .. }) => *s,
		}
	}
}

#[derive(Debug, Clone)]
pub struct ElementWrapper<'a> {
	pub id: taffy::NodeId,
	pub el: &'a Element,
}

#[allow(unused)]
impl<'a> ElementWrapper<'a> {
	pub fn name(&'a self) -> &'a QualName {
		&self.el.name
	}

	pub fn namespace(&'a self) -> &'a Namespace {
		&self.el.name.ns
	}

	pub fn local_name(&'a self) -> &'a LocalName {
		&self.el.name.local
	}

	pub fn attr_ns(&'a self, ns: Namespace, name: LocalName) -> Option<&'a str> {
		self.el.attrs.get(&(ns, name)).map(|v| v.as_str())
	}
}

#[derive(Debug, Clone)]
pub struct TextWrapper<'a> {
	pub id: taffy::NodeId,
	pub t: &'a Text,
}

impl<'a> TextWrapper<'a> {
	pub fn text(&'a self) -> &'a str {
		&self.t.t
	}
}

pub enum EdgeRef<'a> {
	OpenElement(ElementWrapper<'a>),
	CloseElement(taffy::NodeId),
	Text(TextWrapper<'a>),
}

pub struct NodeTreeIter<'a> {
	tree: &'a taffy::TaffyTree<Node>,
	stack: Vec<EdgeRef<'a>>,
}

impl<'a> NodeTreeIter<'a> {
	pub fn new(tree: &'a taffy::TaffyTree<Node>, id: taffy::NodeId) -> Self {
		let stack = Self::child_edges_rev(tree, id).collect();
		Self { tree, stack }
	}

	fn child_edges_rev(
		tree: &'a taffy::TaffyTree<Node>,
		id: taffy::NodeId,
	) -> impl Iterator<Item = EdgeRef<'a>> {
		tree.children(id)
			.unwrap_or_default()
			.into_iter()
			.rev()
			.filter_map(|child| match tree.get_node_context(child) {
				Some(Node::Element(el)) => {
					Some(EdgeRef::OpenElement(ElementWrapper { id: child, el }))
				}
				Some(Node::Text(t)) => Some(EdgeRef::Text(TextWrapper { id: child, t })),
				None => None,
			})
	}
}

impl<'a> Iterator for NodeTreeIter<'a> {
	type Item = EdgeRef<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		let node = self.stack.pop()?;
		if let EdgeRef::OpenElement(ref el) = node {
			self.stack.push(EdgeRef::CloseElement(el.id));
			self.stack.extend(Self::child_edges_rev(self.tree, el.id));
		}
		Some(node)
	}
}

pub struct NodeTree {
	pub(crate) tree: taffy::TaffyTree<Node>,
	pub(crate) root: taffy::NodeId,
	pub(crate) body: Option<taffy::NodeId>,
	#[allow(unused)]
	pub(crate) error: taffy::NodeId,
	pub(crate) parse_errors: Vec<Cow<'static, str>>,
}

impl NodeTree {
	pub fn nodes<'a>(&'a self) -> NodeTreeIter<'a> {
		NodeTreeIter::new(&self.tree, self.root)
	}

	pub fn body_nodes<'a>(&'a self) -> Option<NodeTreeIter<'a>> {
		self.body.map(|body| NodeTreeIter::new(&self.tree, body))
	}

	pub fn into_builder(self) -> Result<NodeTreeBuilder, TreeBuilderError> {
		let NodeTree {
			mut tree,
			mut parse_errors,
			..
		} = self;
		tree.clear();
		let root = tree.new_leaf(Style::default())?;
		let error = tree.new_leaf(Style::default())?;
		parse_errors.clear();

		Ok(NodeTreeBuilder {
			tree: tree.into(),
			root,
			body: Cell::new(None),
			error,
			parse_errors: parse_errors.into(),
		})
	}
}

impl From<NodeTreeBuilder> for NodeTree {
	fn from(value: NodeTreeBuilder) -> Self {
		let NodeTreeBuilder {
			tree,
			root,
			body,
			error,
			parse_errors,
		} = value;
		Self {
			tree: tree.into_inner(),
			root,
			body: body.into_inner(),
			error,
			parse_errors: parse_errors.into_inner(),
		}
	}
}

pub struct NodeTreeBuilder {
	tree: RefCell<taffy::TaffyTree<Node>>,
	root: taffy::NodeId,
	body: Cell<Option<taffy::NodeId>>,
	error: taffy::NodeId,
	parse_errors: RefCell<Vec<Cow<'static, str>>>,
}

impl NodeTreeBuilder {
	pub fn new() -> Result<Self, TreeBuilderError> {
		let mut tree = TaffyTree::new();
		let root = tree.new_leaf(Style::default())?;
		let error = tree.new_leaf(Style::default())?;
		let parse_errors = Vec::new().into();

		Ok(Self {
			tree: tree.into(),
			root,
			body: Cell::new(None),
			error,
			parse_errors,
		})
	}

	pub fn read_from<R: io::Read>(self, mut reader: R) -> Result<NodeTree, TreeBuilderError> {
		let parser = html5ever::parse_document(self, Default::default());
		let tree = Utf8LossyDecoder::new(parser).read_from(&mut reader)?;
		Ok(tree)
	}
}

impl TreeSink for NodeTreeBuilder {
	type Handle = taffy::NodeId;
	type Output = NodeTree;
	type ElemName<'a> = Ref<'a, QualName>;

	fn finish(self) -> Self::Output {
		self.into()
	}

	fn parse_error(&self, msg: Cow<'static, str>) {
		log::trace!("Error during parse: {msg}");
		self.parse_errors.borrow_mut().push(msg);
	}

	fn get_document(&self) -> Self::Handle {
		self.root
	}

	fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
		Ref::map(self.tree.borrow(), |nodes| {
			match &nodes.get_node_context(*target) {
				Some(Node::Element(element)) => &element.name,
				_ => panic!("Not element node: {target:?}"),
			}
		})
	}

	fn create_element(
		&self,
		name: QualName,
		attrs: Vec<Attribute>,
		_flags: html5ever::interface::ElementFlags,
	) -> Self::Handle {
		log::trace!("create_element({name:?}, {attrs:?})");
		let is_body = name.local == local_name!("body");
		let attrs = attrs
			.into_iter()
			.map(|a| ((a.name.ns, a.name.local), a.value.to_string()))
			.collect();
		match self
			.tree
			.borrow_mut()
			.new_leaf_with_context(Style::default(), Node::Element(Element { name, attrs }))
		{
			Ok(node) => {
				if is_body {
					self.body.set(Some(node));
				}
				node
			}
			Err(e) => {
				log::error!("error in create_element: {e}");
				self.error
			}
		}
	}

	fn create_comment(&self, text: html5ever::tendril::StrTendril) -> Self::Handle {
		log::trace!("create_comment('{text}')");
		match self.tree.borrow_mut().new_leaf(Style {
			display: taffy::Display::None,
			..Default::default()
		}) {
			Ok(node) => node,
			Err(e) => {
				log::error!("error in create_comment({text}): {e}");
				self.error
			}
		}
	}

	fn create_pi(
		&self,
		target: html5ever::tendril::StrTendril,
		data: html5ever::tendril::StrTendril,
	) -> Self::Handle {
		log::trace!("create_pi({target}, {data})");
		match self.tree.borrow_mut().new_leaf(Style {
			display: taffy::Display::None,
			..Default::default()
		}) {
			Ok(node) => node,
			Err(e) => {
				log::error!("error in create_pi({target}, {data}): {e}");
				self.error
			}
		}
	}

	fn append_doctype_to_document(
		&self,
		name: html5ever::tendril::StrTendril,
		public_id: html5ever::tendril::StrTendril,
		system_id: html5ever::tendril::StrTendril,
	) {
		log::trace!("append_doctype_to_document({name}, {public_id}, {system_id})");
	}

	fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
		match self.append_safe(parent, child) {
			Ok(_) => {}
			Err(e) => log::error!("error in append({parent:?}, NodeOrText): {e}"),
		}
	}

	fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
		match self.append_before_sibling_safe(sibling, new_node) {
			Ok(_) => {}
			Err(e) => log::error!("error in append_before_sibling({sibling:?}, NodeOrText): {e}"),
		}
	}

	fn append_based_on_parent_node(
		&self,
		element: &Self::Handle,
		prev_element: &Self::Handle,
		child: html5ever::interface::NodeOrText<Self::Handle>,
	) {
		log::trace!("append_based_on_parent_node({element:?}, {prev_element:?}, child)");
		if self.tree.borrow().parent(*element).is_some() {
			self.append_before_sibling(element, child)
		} else {
			self.append(prev_element, child)
		}
	}

	fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
		match self.tree.borrow().children(*target) {
			Ok(children) => children.first().cloned().unwrap_or(self.error),
			Err(e) => {
				log::error!("error in get_template_content({target:?}): {e}");
				self.error
			}
		}
	}

	fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
		x == y
	}

	fn set_quirks_mode(&self, _mode: html5ever::interface::QuirksMode) {}

	fn add_attrs_if_missing(&self, target: &Self::Handle, add_attrs: Vec<html5ever::Attribute>) {
		log::trace!("add_attrs_if_missing({target:?}, {add_attrs:?})");
		let mut tree = self.tree.borrow_mut();
		let Some(Node::Element(Element { attrs, .. })) = tree.get_node_context_mut(*target) else {
			panic!("Promise violated, no element exists");
		};
		for attr in add_attrs {
			attrs
				.entry((attr.name.ns, attr.name.local))
				.or_insert_with(|| attr.value.to_string());
		}
	}

	fn remove_from_parent(&self, target: &Self::Handle) {
		log::trace!("remove_from_parent({target:?})");
		match self.remove_from_parent_safe(target) {
			Ok(_) => {}
			Err(e) => log::error!("error in remove_from_parent({target:?}): {e}"),
		}
	}

	fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
		log::trace!("reparent_children({node:?}, {new_parent:?})");
		match self.reparent_children_safe(node, new_parent) {
			Ok(_) => {}
			Err(e) => log::error!("error in reparent_children({node:?}, {new_parent:?}): {e}"),
		}
	}
}

impl NodeTreeBuilder {
	fn append_safe(
		&self,
		parent: &taffy::NodeId,
		child: NodeOrText<taffy::NodeId>,
	) -> Result<(), TreeBuilderError> {
		let parent = *parent;
		let mut tree = self.tree.borrow_mut();
		match child {
			NodeOrText::AppendNode(node_id) => {
				log::trace!("append({parent:?}, {node_id:?})");
				tree.add_child(parent, node_id)?;
			}
			NodeOrText::AppendText(t) => {
				log::trace!("append({parent:?}, '{t}')");
				if t.trim().is_empty() {
					return Ok(());
				}
				let last_child = tree.children(parent).ok().and_then(|v| v.last().cloned());
				if let Some(node_id) = last_child {
					match tree.get_node_context_mut(node_id) {
						Some(Node::Element(_)) | None => {
							let style = tree
								.get_node_context(parent)
								.map(|el| el.get_style())
								.unwrap_or_default();
							let node = tree.new_leaf_with_context(
								Style::default(),
								Node::Text(Text { style, t }),
							)?;
							tree.add_child(parent, node)?;
						}
						Some(Node::Text(Text { t: text, .. })) => {
							text.push_tendril(&t);
						}
					}
				} else {
					let style = tree
						.get_node_context(parent)
						.map(|el| el.get_style())
						.unwrap_or_default();
					let node = tree
						.new_leaf_with_context(Style::default(), Node::Text(Text { style, t }))?;
					tree.add_child(parent, node)?;
				}
			}
		}
		Ok(())
	}

	fn append_before_sibling_safe(
		&self,
		sibling: &taffy::NodeId,
		new_node: NodeOrText<taffy::NodeId>,
	) -> Result<(), TreeBuilderError> {
		let mut tree = self.tree.borrow_mut();
		let parent = tree
			.parent(*sibling)
			.ok_or(TreeBuilderError::SiblingHasNoParent)?;
		let sibling_index = tree
			.children(parent)?
			.iter()
			.position(|id| id == sibling)
			.unwrap_or(0);

		match new_node {
			NodeOrText::AppendNode(node) => {
				log::trace!("append_before_sibling({sibling:?}, {node:?})");
				if let Some(old_parent) = tree.parent(node) {
					tree.remove_child(old_parent, node)?;
				}
				tree.insert_child_at_index(parent, sibling_index, node)?;
			}
			NodeOrText::AppendText(t) => {
				log::trace!("append_before_sibling({sibling:?}, '{t}')");
				if t.trim().is_empty() {
					return Ok(());
				}
				if let Ok(node) = tree.child_at_index(parent, sibling_index - 1)
					&& let Some(Node::Text(Text { t: tendril, .. })) =
						tree.get_node_context_mut(node)
				{
					tendril.push_tendril(&t);
				} else {
					let style = tree
						.get_node_context(parent)
						.map(|el| el.get_style())
						.unwrap_or_default();
					let node = tree
						.new_leaf_with_context(Style::default(), Node::Text(Text { style, t }))?;
					tree.insert_child_at_index(parent, sibling_index, node)?;
				}
			}
		};
		Ok(())
	}

	fn reparent_children_safe(
		&self,
		node: &taffy::NodeId,
		new_parent: &taffy::NodeId,
	) -> Result<(), TreeBuilderError> {
		let mut tree = self.tree.borrow_mut();
		let children = tree.children(*node)?;
		for child in children {
			tree.remove_child(*node, child)?;
			tree.add_child(*new_parent, child)?;
		}
		Ok(())
	}

	fn remove_from_parent_safe(&self, target: &taffy::NodeId) -> Result<(), TreeBuilderError> {
		let mut tree = self.tree.borrow_mut();
		if let Some(parent) = tree.parent(*target) {
			tree.remove_child(parent, *target)?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use html5ever::parse_document;
	use html5ever::tendril::TendrilSink;

	use crate::html_parser::EdgeRef;
	use crate::html_parser::Node;
	use crate::html_parser::NodeTreeBuilder;

	#[test]
	fn test_html_parser_text() {
		let _ = env_logger::try_init();
		let input = "testing";

		let parser = parse_document(NodeTreeBuilder::new().unwrap(), Default::default());
		let tree = parser.one(input);

		let children = tree.tree.children(tree.root).unwrap();
		let html_id = children
			.into_iter()
			.find(|n| matches!(tree.tree.get_node_context(*n), Some(Node::Element(element)) if &element.name.local == "html"))
			.expect("Missing html element");

		let children = tree.tree.children(html_id).unwrap();
		children
			.iter()
			.find(|&n| matches!(tree.tree.get_node_context(*n), Some(Node::Element(element)) if &element.name.local == "head"))
			.expect("Missing head element");
		children
			.iter()
			.find(|&n| matches!(tree.tree.get_node_context(*n), Some(Node::Element(element)) if &element.name.local == "body"))
			.expect("Missing body element");

		assert_eq!(
			tree.tree.total_node_count(),
			6,
			"Unpected amount of nodes in tree"
		);
	}

	#[test]
	fn test_html_parser_html() {
		let _ = env_logger::try_init();
		let input = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>title</title>
    <link rel="stylesheet" href="style.css">
    <script src="script.js"></script>
  </head>
  <body>
    <!-- page content -->
  </body>
</html>
"#;

		let parser = parse_document(NodeTreeBuilder::new().unwrap(), Default::default());
		parser.one(input);
	}

	#[test]
	fn test_html_parser_basic_iter() {
		let _ = env_logger::try_init();
		let input = "testing";

		let parser = parse_document(NodeTreeBuilder::new().unwrap(), Default::default());
		let node_tree = parser.one(input);

		let mut has_html = false;
		let mut has_head = false;
		let mut has_body = false;

		for n in node_tree.nodes() {
			match n {
				EdgeRef::OpenElement(el) => {
					if el.local_name() == "html" {
						has_html = true;
					}
					if el.local_name() == "head" {
						has_head = true;
					}
					if el.local_name() == "body" {
						has_body = true;
					}
				}
				EdgeRef::CloseElement(_) => {}
				EdgeRef::Text(text) => {
					let s = text.text();
					assert_eq!(s, input, "Unexpected text content");
				}
			}
		}

		assert!(has_html, "Missing html element");
		assert!(has_head, "Missing head element");
		assert!(has_body, "Missing body element");
	}
}
