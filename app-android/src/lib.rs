#![cfg(target_os = "android")]

use std::fs;
use std::io;
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use android_logger::Config;
use jni::Env;
use jni::native_method;
use jni::objects::JObject;
use jni::objects::JString;
use jni::refs::Global;
use jni::sys::jint;
use jni::sys::jlong;
use jni::vm::JavaVM;
use scribble_reader::start;
use scribe::ScribeConfig;
use scribe::settings;
use scribe::settings::Paths;
use winit::event_loop::EventLoop;
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::platform::android::activity::AndroidApp;
use winit::platform::android::activity::OnCreateState;
use wrangler::Discovery;
use wrangler::DiscoveryDocument;
use wrangler::DocumentId;
use wrangler::DocumentTree;
use wrangler::FileContent;
use wrangler::Ticket;
use wrangler::Wrangler;
use wrangler::WranglerCommand;
use wrangler::WranglerResult;
use wrangler::WranglerSystem;

jni::bind_java_type! { Context => "android.content.Context" }
jni::bind_java_type! { Activity => "android.app.Activity" }
jni::bind_java_type! {
	MainActivity => "org.lotrax.scribblereader.MainActivity",
	type_map {
		Context => "android.content.Context",
		Activity => "android.app.Activity",
	},
	methods {
		fn discover_open_tree(),
		fn discover_folder_content(ticket_id: i64, root_uri: JString),
		fn open_file_content(ticket_id: i64, root_uri: JString, doc_id: JString),
	}
}

const _WRANGLER_NATIVE_METHODS: &[jni::NativeMethod] = &[
	native_method! {
		java_type = "org.lotrax.scribblereader.MainActivity",
		extern fn wrangler_open_tree(root_uri: JString),
	},
	native_method! {
		java_type = "org.lotrax.scribblereader.MainActivity",
		extern fn wrangler_discover_start(ticket_id: jlong),
	},
	native_method! {
		java_type = "org.lotrax.scribblereader.MainActivity",
		extern fn wrangler_discover_end(ticket_id: jlong),
	},
	native_method! {
		java_type = "org.lotrax.scribblereader.MainActivity",
		extern fn wrangler_discover(ticket_id: jlong, doc_id: JString, name: JString, size: jlong, last_modified: jlong),
	},
	native_method! {
		java_type = "org.lotrax.scribblereader.MainActivity",
		extern fn wrangler_file(ticket_id: jlong, doc_id: JString, fd: jint, size: jlong, last_modified: jlong),
	},
	native_method! {
		java_type = "org.lotrax.scribblereader.MainActivity",
		extern fn wrangler_fail(ticket_id: jlong, reason: JString),
	},
];

static WRANGLERS: OnceLock<Arc<Mutex<Vec<Box<dyn Wrangler>>>>> = OnceLock::new();

static WRANGLER_SENDER: OnceLock<Sender<WranglerCommand>> = OnceLock::new();

fn wrangler_open_tree<'local>(
	env: &mut Env<'local>,
	_this: JObject<'local>,
	root_uri: JString,
) -> Result<(), jni::errors::Error> {
	let root_uri = Arc::new(root_uri.try_to_string(env)?);
	log::info!("Got root_uri: {}", &root_uri);
	let tree = DocumentTree::new(root_uri);

	// Preferrably, this should be inited elsewhere
	let sender = WRANGLER_SENDER.wait();
	sender.send(WranglerCommand::SetTree(tree)).unwrap();

	Ok(())
}

fn wrangler_discover_start<'local>(
	_env: &mut Env<'local>,
	_this: JObject<'local>,
	ticket_id: jlong,
) -> Result<(), jni::errors::Error> {
	let ticket = Ticket::new(ticket_id as u64);
	let mut handled = false;
	for w in &mut *WRANGLERS.wait().lock().unwrap() {
		let result = w.discover(ticket, Discovery::Begin);
		if matches!(result, WranglerResult::Handled) {
			handled = true;
			break;
		}
	}
	if !handled {
		log::info!("wrangler_discover_start unhandled {ticket:?}");
	}
	Ok(())
}

fn wrangler_discover_end<'local>(
	_env: &mut Env<'local>,
	_this: JObject<'local>,
	ticket_id: jlong,
) -> Result<(), jni::errors::Error> {
	let ticket = Ticket::new(ticket_id as u64);
	let mut handled = false;
	for w in &mut *WRANGLERS.wait().lock().unwrap() {
		let result = w.discover(ticket, Discovery::End);
		if matches!(result, WranglerResult::Handled) {
			handled = true;
			break;
		}
	}
	if !handled {
		log::info!("wrangler_discover_start unhandled {ticket:?}");
	}
	Ok(())
}

fn wrangler_discover<'local>(
	env: &mut Env<'local>,
	_this: JObject<'local>,
	ticket_id: jlong,
	doc_id: JString,
	name: JString,
	size: jlong,
	last_modified: jlong,
) -> Result<(), jni::errors::Error> {
	let ticket = Ticket::new(ticket_id as u64);
	let doc_id = doc_id.try_to_string(env)?;
	let name = name.try_to_string(env)?;
	let size = size as u64;
	let timestamp = UNIX_EPOCH + Duration::from_millis(last_modified as u64);

	let document = DiscoveryDocument {
		document: DocumentId::new(doc_id),
		file_name: &name,
		size,
		timestamp,
	};

	let mut handled = false;
	for w in &mut *WRANGLERS.wait().lock().unwrap() {
		let result = w.discover(ticket, Discovery::Document(&document));
		if matches!(result, WranglerResult::Handled) {
			handled = true;
			break;
		}
	}
	if !handled {
		log::info!("wrangler_discover_start unhandled {ticket:?}");
	}

	Ok(())
}

fn wrangler_file<'local>(
	env: &mut Env<'local>,
	_this: JObject<'local>,
	ticket_id: jlong,
	doc_id: JString,
	fd: jint,
	size: jlong,
	last_modified: jlong,
) -> Result<(), jni::errors::Error> {
	let ticket = Ticket::new(ticket_id as u64);
	let doc_id = doc_id.try_to_string(env)?;
	let file = ManuallyDrop::new(unsafe { fs::File::from_raw_fd(fd) });
	let size = size as u64;
	let timestamp = UNIX_EPOCH + Duration::from_millis(last_modified as u64);

	let result = Ok(FileContent {
		document: DocumentId::new(doc_id),
		size,
		timestamp,
		file: &file,
	});

	let mut handled = false;
	for w in &mut *WRANGLERS.wait().lock().unwrap() {
		let result = w.file(ticket, &result);
		if matches!(result, WranglerResult::Handled) {
			handled = true;
			break;
		}
	}
	if !handled {
		log::info!("wrangler_discover_start unhandled {ticket:?}");
	}

	Ok(())
}

fn wrangler_fail<'local>(
	env: &mut Env<'local>,
	_this: JObject<'local>,
	ticket_id: jlong,
	reason: JString,
) -> Result<(), jni::errors::Error> {
	let ticket = Ticket::new(ticket_id as u64);
	let reason = reason.try_to_string(env)?;

	send_document_fail(ticket, reason);
	Ok(())
}

fn send_document_fail(ticket: Ticket, err: impl ToString) {
	let error = Err(io::Error::new(io::ErrorKind::Other, err.to_string()));

	let mut handled = false;
	for w in &mut *WRANGLERS.wait().lock().unwrap() {
		let result = w.file(ticket, &error);
		if matches!(result, WranglerResult::Handled) {
			handled = true;
			break;
		}
	}
	if !handled {
		log::info!("wrangler_discover_start unhandled {ticket:?}");
	}
}

fn create_wrangler(app: &AndroidApp, config: ScribeConfig) -> (WranglerSystem, JoinHandle<()>) {
	let (sender, receiver) = channel();
	let wranglers = WRANGLERS
		.get_or_init(|| Arc::new(Mutex::new(Vec::new())))
		.clone();

	WRANGLER_SENDER.get_or_init(|| sender.clone());

	let system = WranglerSystem::new(sender, wranglers.clone());

	let app = app.clone();
	let handle = thread::spawn(move || {
		let vm = match JavaVM::singleton() {
			Ok(vm) => vm,
			Err(e) => {
				log::error!("Failed to get vm: {e}");
				return;
			}
		};
		let activity: jni::sys::jobject = app.activity_as_ptr() as _;

		let mut document_tree = loop {
			match receiver.recv() {
				Ok(WranglerCommand::SetTree(tree)) => {
					match config.clone().set_library(settings::Library {
						path: Some(tree.clone().into_inner()),
					}) {
						Ok(_) => {}
						Err(e) => {
							log::error!("Failed to save config: {e}");
						}
					};
					log::info!("Set tree {tree}");
					break tree;
				}
				Ok(WranglerCommand::Shutdown) => {
					log::info!("Wrangler shutdown");
					return;
				}
				Ok(cmd) => {
					log::warn!("Wrangler not initialized, ignore cmd: {cmd:?}");
				}
				Err(e) => {
					log::error!("Wrangler error, exit: {e}");
					return;
				}
			}
		};
		for cmd in receiver.into_iter() {
			match cmd {
				WranglerCommand::SetTree(tree) => {
					log::info!("Set tree {tree}");
					document_tree = tree;
				}
				WranglerCommand::Document(ticket, doc_id) => {
					if let Err(e) = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
						let activity =
							unsafe { env.as_cast_raw::<Global<MainActivity>>(&activity)? };

						let root_uri = JString::from_str(env, &document_tree)?;
						let ticket_id = ticket.value() as jlong;
						let doc_id = JString::from_str(env, &doc_id)?;

						MainActivity::open_file_content(
							activity.as_ref(),
							env,
							ticket_id,
							root_uri,
							doc_id,
						)?;

						Ok(())
					}) {
						log::error!("Failed to interact with vm: {e}");
						send_document_fail(ticket, e);
						break;
					}
				}
				WranglerCommand::ExploreTree(ticket) => {
					if let Err(e) = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
						let activity =
							unsafe { env.as_cast_raw::<Global<MainActivity>>(&activity)? };

						let root_uri = JString::from_str(env, &document_tree)?;
						let ticket_id = ticket.value() as jlong;

						MainActivity::discover_folder_content(
							activity.as_ref(),
							env,
							ticket_id,
							root_uri,
						)?;

						Ok(())
					}) {
						log::error!("Failed to interact with vm: {e}");
						for w in &mut *WRANGLERS.wait().lock().unwrap() {
							let result = w.discover(ticket, Discovery::End);
							if matches!(result, WranglerResult::Handled) {
								break;
							}
						}
						break;
					}
				}
				WranglerCommand::Shutdown => {
					log::info!("Wrangler shutdown");
					return;
				}
			}
		}
	});

	(system, handle)
}

#[unsafe(no_mangle)]
fn android_on_create(state: &OnCreateState) {
	let vm = unsafe { JavaVM::from_raw(state.vm_as_ptr().cast()) };
	if let Err(err) = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
		log::info!("Initialize jni buindings");
		let _ = ContextAPI::get(env, &Default::default())?;
		let _ = ActivityAPI::get(env, &Default::default())?;
		let _ = MainActivityAPI::get(env, &Default::default())?;
		Ok(())
	}) {
		log::error!("Failed to interact with on jvm: {err:?}");
	}
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
	android_logger::init_once(
		Config::default()
			.with_tag("scribble-reader")
			.with_max_level(log::LevelFilter::Trace)
			.with_filter(
				android_logger::FilterBuilder::new()
					.parse("info,naga=warn,wgpu=warn,scribe=trace")
					.build(),
			),
	);

	let ext_data_path = app.external_data_path().unwrap();
	let paths = Paths {
		cache_path: Arc::new(ext_data_path.parent().unwrap().join("cache")),
		config_path: Arc::new(ext_data_path.join("config")),
		data_path: Arc::new(ext_data_path.join("data")),
	};
	let config = ScribeConfig::new(Arc::new(paths));

	let (system, handle) = create_wrangler(&app, config.clone());

	let library = match config.library() {
		Ok(l) => l,
		Err(e) => {
			log::error!("Failed to read library config: {e}");
			return;
		}
	};
	if let Some(tree) = library.path {
		system.send(WranglerCommand::SetTree(DocumentTree::new(tree)));
	} else {
		let vm = match JavaVM::singleton() {
			Ok(vm) => vm,
			Err(e) => {
				log::error!("Failed to get vm: {e}");
				return;
			}
		};
		let activity: jni::sys::jobject = app.activity_as_ptr() as _;
		if let Err(e) = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
			let activity = unsafe { env.as_cast_raw::<Global<MainActivity>>(&activity)? };
			MainActivity::discover_open_tree(activity.as_ref(), env)?;
			Ok(())
		}) {
			log::error!("Failed to interact with vm: {e}");
		}
	};

	let event_loop = EventLoop::with_user_event()
		.with_android_app(app)
		.build()
		.unwrap();

	if let Err(e) = start(config, system.clone(), event_loop) {
		log::error!("App error: {e}")
	};

	system.send(wrangler::WranglerCommand::Shutdown);
	handle.join().unwrap();
}
