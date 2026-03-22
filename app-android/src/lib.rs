#![cfg(target_os = "android")]

use android_logger::Config;
use jni::jni_sig;
use jni::jni_str;
use jni::objects::JObject;
use jni::sys::jobject;
use scribble_reader::start;
use scribe::ScribeConfig;
use scribe::settings::Paths;
use winit::event_loop::EventLoop;
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::platform::android::activity::AndroidApp;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
	android_logger::init_once(
		Config::default()
			.with_tag("scribble-reader")
			.with_max_level(log::LevelFilter::Info)
			.with_filter(
				android_logger::FilterBuilder::new()
					.parse("debug,naga=warn,wgpu=warn")
					.build(),
			),
	);

	let ext_data_path = app.external_data_path().unwrap();
	let paths = Paths {
		cache_path: ext_data_path.parent().unwrap().join("cache"),
		config_path: ext_data_path.join("config"),
		data_path: ext_data_path.join("data"),
	};
	let config = ScribeConfig::new(paths);

	match call_request_open_folder_tree(&app) {
		Ok(_) => {}
		Err(e) => log::error!("Error: {e}"),
	}

	let event_loop = EventLoop::with_user_event()
		.with_android_app(app)
		.build()
		.unwrap();

	match start(event_loop, config) {
		Ok(_) => {}
		Err(e) => log::error!("Error: {e}"),
	}
}

#[derive(Debug, thiserror::Error)]
enum AndroidPermissionErrors {
	#[error("Unknown error")]
	Jni(#[from] jni::errors::Error),
}

fn call_request_open_folder_tree(
	app: &winit::platform::android::activity::AndroidApp,
) -> Result<(), AndroidPermissionErrors> {
	let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
	vm.attach_current_thread(|env| -> jni::errors::Result<()> {
		let activity = unsafe { JObject::from_raw(&env, app.activity_as_ptr() as jobject) };

		env.call_method(
			&activity,
			jni_str!("requestOpenFolderTree"),
			jni_sig!("()V"),
			&[],
		)?;
		Ok(())
	})?;

	log::info!("Call check self permissions");

	Ok(())
}
