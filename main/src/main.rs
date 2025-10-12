use scribble_reader::start;
use scribe::Settings;
use winit::event_loop::EventLoop;

fn main() {
	let _ = dotenv::dotenv();

	env_logger::builder()
		.filter_level(log::LevelFilter::Info)
		.parse_default_env()
		.init();

	let xdg_dirs = xdg::BaseDirectories::with_prefix("scribble-reader");
	let s = Settings {
		cache_path: xdg_dirs.get_cache_home().unwrap(),
		config_path: xdg_dirs.get_config_home().unwrap(),
		data_path: xdg_dirs.get_data_home().unwrap(),
	};

	let event_loop = EventLoop::with_user_event().build().unwrap();
	match start(event_loop, s) {
		Ok(_) => {}
		Err(e) => log::error!("Error: {e}"),
	}
}
