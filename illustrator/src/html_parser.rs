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

#[derive(Debug, thiserror::Error)]
pub enum TreeBuilderError {
	#[error(transparent)]
	Io(#[from] io::Error),
}

#[derive(Debug)]
pub struct Element {
	pub name: QualName,
	pub attrs: BTreeMap<(Namespace, LocalName), String>,
}

#[derive(Debug)]
pub struct Text {
	pub t: StrTendril,
}

#[derive(Debug)]
pub(crate) enum Leaf {
	Element(Element),
	Text(Text),
}

#[derive(Debug, Clone)]
pub struct ElementWrapper<'a> {
	pub id: NodeId,
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
	#[allow(dead_code)]
	pub id: NodeId,
	pub t: &'a Text,
}

pub enum EdgeRef<'a> {
	OpenElement(ElementWrapper<'a>),
	CloseElement(NodeId, LocalName),
	Text(TextWrapper<'a>),
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
	pub(crate) fn value(&self) -> u32 {
		let NodeId(v) = self;
		*v
	}
}

#[derive(Debug)]
pub(crate) struct Tree<T> {
	node_id_counter: u32,
	child_map: BTreeMap<NodeId, Vec<NodeId>>,
	parent_map: BTreeMap<NodeId, NodeId>,
	contexts: BTreeMap<NodeId, T>,
}

impl<T> Tree<T> {
	pub(crate) fn new() -> Self {
		Self {
			node_id_counter: 0,
			child_map: BTreeMap::new(),
			parent_map: BTreeMap::new(),
			contexts: BTreeMap::new(),
		}
	}

	#[allow(dead_code)]
	pub(crate) fn node_count(&self) -> u32 {
		self.node_id_counter
	}

	pub(crate) fn parent(&self, id: NodeId) -> Option<NodeId> {
		self.parent_map.get(&id).cloned()
	}

	pub(crate) fn children(&self, id: NodeId) -> Option<&[NodeId]> {
		self.child_map.get(&id).map(|v| v.as_slice())
	}

	pub(crate) fn get_context(&self, id: NodeId) -> Option<&T> {
		self.contexts.get(&id)
	}

	pub(crate) fn get_context_mut(&mut self, id: NodeId) -> Option<&mut T> {
		self.contexts.get_mut(&id)
	}

	pub(crate) fn remove_parent(&mut self, id: NodeId) {
		let existing = self.parent_map.get(&id).cloned();
		if let Some(parent_id) = existing {
			self.child_map.entry(parent_id).and_modify(|v| {
				if let Some(i) = v.iter().position(|c| *c == id) {
					v.remove(i);
				}
			});
		}
	}

	pub(crate) fn add_node(&mut self) -> NodeId {
		let id = NodeId(self.node_id_counter);
		self.node_id_counter += 1;
		id
	}

	pub(crate) fn add_node_with_context(&mut self, context: T) -> NodeId {
		let id = NodeId(self.node_id_counter);
		self.node_id_counter += 1;
		self.contexts.insert(id, context);
		id
	}

	pub(crate) fn add_child(&mut self, parent_id: NodeId, child_id: NodeId) {
		debug_assert_ne!(parent_id, child_id, "Tried to append node to self");
		self.remove_parent(child_id);
		self.parent_map.insert(child_id, parent_id);
		self.child_map.entry(parent_id).or_default().push(child_id);
	}

	pub(crate) fn add_child_before_sibling(&mut self, sibling_id: NodeId, child_id: NodeId) {
		let parent_id = self
			.parent_map
			.get(&sibling_id)
			.cloned()
			.expect("Expected sibling to have parent");
		self.remove_parent(child_id);
		self.parent_map.insert(child_id, parent_id);

		let children = self.child_map.entry(parent_id).or_default();
		if let Some(i) = children.iter().position(|c| *c == sibling_id) {
			children.insert(i, child_id);
		} else {
			children.push(child_id);
		}
	}

	pub(crate) fn clear(&mut self) {
		self.node_id_counter = 0;
		self.parent_map.clear();
		self.child_map.clear();
		self.contexts.clear();
	}
}

pub struct NodeTreeBuilder {
	root: NodeId,
	error: NodeId,
	body: Cell<Option<NodeId>>,
	tree: RefCell<Tree<Leaf>>,
	parse_errors: RefCell<Vec<Cow<'static, str>>>,
}

impl NodeTreeBuilder {
	pub fn new() -> Self {
		let parse_errors = Vec::new().into();
		let mut tree = Tree::new();
		let root = tree.add_node();
		let body = Cell::new(None);
		let error = tree.add_node();
		let tree = RefCell::new(tree);

		Self {
			root,
			error,
			body,
			tree,
			parse_errors,
		}
	}

	pub fn read_from<R: io::Read>(self, mut reader: R) -> Result<NodeTreeResult, TreeBuilderError> {
		let parser = html5ever::parse_document(self, Default::default());
		let tree = Utf8LossyDecoder::new(parser).read_from(&mut reader)?;
		Ok(tree)
	}
}

pub struct NodeTreeIter<'a> {
	tree: &'a Tree<Leaf>,
	stack: Vec<EdgeRef<'a>>,
}

impl<'a> NodeTreeIter<'a> {
	pub fn new(tree: &'a Tree<Leaf>, id: NodeId) -> Self {
		let stack = Self::child_edges_rev(tree, id).collect();
		Self { tree, stack }
	}

	fn child_edges_rev(tree: &'a Tree<Leaf>, id: NodeId) -> impl Iterator<Item = EdgeRef<'a>> {
		tree.children(id)
			.unwrap_or_default()
			.iter()
			.rev()
			.filter_map(|child| match tree.get_context(*child) {
				Some(Leaf::Element(el)) => {
					Some(EdgeRef::OpenElement(ElementWrapper { id: *child, el }))
				}
				Some(Leaf::Text(t)) => Some(EdgeRef::Text(TextWrapper { id: *child, t })),
				None => None,
			})
	}
}

impl<'a> Iterator for NodeTreeIter<'a> {
	type Item = EdgeRef<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		let node = self.stack.pop()?;
		if let EdgeRef::OpenElement(ref el) = node {
			self.stack
				.push(EdgeRef::CloseElement(el.id, el.local_name().clone()));
			self.stack.extend(Self::child_edges_rev(self.tree, el.id));
		}
		Some(node)
	}
}

pub struct NodeTreeResult {
	#[allow(dead_code)]
	pub(crate) root: NodeId,
	#[allow(dead_code)]
	pub(crate) error: NodeId,
	pub(crate) body: Option<NodeId>,
	pub(crate) tree: Tree<Leaf>,
	pub(crate) parse_errors: Vec<Cow<'static, str>>,
}

impl NodeTreeResult {
	pub fn body_iter<'a>(&'a self) -> Option<NodeTreeIter<'a>> {
		self.body.map(|id| NodeTreeIter::new(&self.tree, id))
	}

	pub(crate) fn into_builder(self) -> NodeTreeBuilder {
		let NodeTreeResult {
			mut tree,
			mut parse_errors,
			..
		} = self;

		tree.clear();
		parse_errors.clear();

		let root = tree.add_node();
		let error = tree.add_node();
		let body = None.into();
		let tree = tree.into();
		let parse_errors = parse_errors.into();

		NodeTreeBuilder {
			root,
			error,
			body,
			tree,
			parse_errors,
		}
	}
}

impl From<NodeTreeBuilder> for NodeTreeResult {
	fn from(value: NodeTreeBuilder) -> Self {
		let NodeTreeBuilder {
			tree,
			root,
			body,
			error,
			parse_errors,
		} = value;
		Self {
			root,
			error,
			body: body.into_inner(),
			tree: tree.into_inner(),
			parse_errors: parse_errors.into_inner(),
		}
	}
}

impl TreeSink for NodeTreeBuilder {
	type Handle = NodeId;
	type Output = NodeTreeResult;
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
			match &nodes.get_context(*target) {
				Some(Leaf::Element(element)) => &element.name,
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
		let is_body =  matches!(&name.local, &local_name!("body"));
		let attrs = attrs
			.into_iter()
			.map(|a| ((a.name.ns, a.name.local), a.value.to_string()))
			.collect();
		let node_id = self.tree
			.borrow_mut()
			.add_node_with_context(Leaf::Element(Element { name, attrs }));
		if is_body {
			self.body.set(Some(node_id));
		}
		node_id
	}

	fn create_comment(&self, text: html5ever::tendril::StrTendril) -> Self::Handle {
		log::trace!("create_comment('{text}')");
		self.tree.borrow_mut().add_node()
	}

	fn create_pi(
		&self,
		target: html5ever::tendril::StrTendril,
		data: html5ever::tendril::StrTendril,
	) -> Self::Handle {
		log::trace!("create_pi({target}, {data})");
		self.tree.borrow_mut().add_node()
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
		let parent = *parent;
		let mut tree = self.tree.borrow_mut();
		match child {
			NodeOrText::AppendNode(node_id) => {
				log::trace!("append({parent:?}, {node_id:?})");
				tree.add_child(parent, node_id);
			}
			NodeOrText::AppendText(t) => {
				log::trace!("append({parent:?}, '{t}')");
				if t.trim().is_empty() {
					log::trace!("Skip empty text");
					return;
				}
				let last_child = tree.children(parent).and_then(|v| v.last().cloned());
				if let Some(node_id) = last_child {
					match tree.get_context_mut(node_id) {
						Some(Leaf::Element(_)) | None => {
							let node = tree.add_node_with_context(Leaf::Text(Text { t }));
							tree.add_child(parent, node);
						}
						Some(Leaf::Text(Text { t: text, .. })) => {
							text.push_tendril(&t);
						}
					}
				} else {
					let node = tree.add_node_with_context(Leaf::Text(Text { t }));
					tree.add_child(parent, node);
				}
			}
		}
	}

	fn append_before_sibling(&self, sibling: &Self::Handle, new_node: NodeOrText<Self::Handle>) {
		let mut tree = self.tree.borrow_mut();
		match new_node {
			NodeOrText::AppendNode(node) => {
				log::trace!("append_before_sibling({sibling:?}, {node:?})");
				tree.add_child_before_sibling(*sibling, node)
			}
			NodeOrText::AppendText(t) => {
				log::trace!("append_before_sibling({sibling:?}, '{t}')");
				if t.trim().is_empty() {
					log::trace!("Skip empty text");
					return;
				}
				let older_sibling = tree
					.parent(*sibling)
					.and_then(|parent| tree.children(parent))
					.and_then(|children| children.iter().take_while(|c| *c != sibling).last())
					.cloned();
				if let Some(Leaf::Text(Text { t: tendril, .. })) =
					older_sibling.and_then(|id| tree.get_context_mut(id))
				{
					tendril.push_tendril(&t);
				} else {
					let node = tree.add_node_with_context(Leaf::Text(Text { t }));
					tree.add_child_before_sibling(*sibling, node);
				}
			}
		};
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
			Some(children) => children.first().cloned().unwrap_or(self.error),
			None => {
				log::error!("error in get_template_content({target:?}): No children");
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
		let Some(Leaf::Element(Element { attrs, .. })) = tree.get_context_mut(*target) else {
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
		let mut tree = self.tree.borrow_mut();
		tree.remove_parent(*target);
	}

	fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
		log::trace!("reparent_children({node:?}, {new_parent:?})");
		let mut tree = self.tree.borrow_mut();
		let children = tree
			.children(*node)
			.map(|children| children.to_vec())
			.unwrap_or_default();
		for child in children {
			tree.add_child(*new_parent, child);
		}
	}
}

#[cfg(test)]
mod tests {
	use html5ever::parse_document;
	use html5ever::tendril::TendrilSink;

	use crate::html_parser::EdgeRef;
	use crate::html_parser::Leaf;
	use crate::html_parser::NodeTreeBuilder;
	use crate::html_parser::NodeTreeIter;

	#[test]
	fn test_html_parser_text() {
		let _ = env_logger::try_init();
		let input = "testing";

		let parser = parse_document(NodeTreeBuilder::new(), Default::default());
		let tree = parser.one(input);

		let children = tree.tree.children(tree.root).unwrap();
		let html_id = children
			.iter()
			.find(|n| matches!(tree.tree.get_context(**n), Some(Leaf::Element(element)) if &element.name.local == "html"))
			.expect("Missing html element");

		let children = tree.tree.children(*html_id).unwrap();
		children
			.iter()
			.find(|&n| matches!(tree.tree.get_context(*n), Some(Leaf::Element(element)) if &element.name.local == "head"))
			.expect("Missing head element");
		children
			.iter()
			.find(|&n| matches!(tree.tree.get_context(*n), Some(Leaf::Element(element)) if &element.name.local == "body"))
			.expect("Missing body element");

		assert_eq!(
			tree.tree.node_count(),
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

		let parser = parse_document(NodeTreeBuilder::new(), Default::default());
		parser.one(input);
	}

	#[test]
	fn test_html_parser_basic_iter() {
		let _ = env_logger::try_init();
		let input = "testing";

		let parser = parse_document(NodeTreeBuilder::new(), Default::default());
		let node_tree = parser.one(input);

		let mut has_html = false;
		let mut has_head = false;
		let mut has_body = false;

		let node_iter = NodeTreeIter::new(&node_tree.tree, node_tree.root);
		for n in node_iter {
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
				EdgeRef::CloseElement(_, _) => {}
				EdgeRef::Text(crate::html_parser::TextWrapper { t, .. }) => {
					let s: &str = &t.t;
					assert_eq!(s, input, "Unexpected text content");
				}
			}
		}

		assert!(has_html, "Missing html element");
		assert!(has_head, "Missing head element");
		assert!(has_body, "Missing body element");
	}
}
