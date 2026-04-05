use std::sync::Arc;

use scribble_reader::start;
use scribe::ScribeConfig;
use scribe::settings::Paths;
use winit::event_loop::EventLoop;
use wrangler::DocumentTree;
use wrangler::create_wrangler;

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let _ = dotenv::dotenv();

	env_logger::builder()
		.filter_level(log::LevelFilter::Info)
		.parse_default_env()
		.init();

	let xdg_dirs = xdg::BaseDirectories::with_prefix("scribble-reader");
	let paths = Paths {
		cache_path: xdg_dirs.get_cache_home().unwrap().into(),
		config_path: xdg_dirs.get_config_home().unwrap().into(),
		data_path: xdg_dirs.get_data_home().unwrap().into(),
	};
	let config = ScribeConfig::new(Arc::new(paths));

	let lib_path = config
		.library()?
		.path
		.unwrap_or(Arc::new("~/Documents/ebook/".to_string()))
		.to_string();
	let (system, handle) = create_wrangler(DocumentTree::new(Arc::new(lib_path)));

	let event_loop = EventLoop::with_user_event().build().unwrap();

	start(config, system.clone(), event_loop)?;

	system.send(wrangler::WranglerCommand::Shutdown);
	handle.join().unwrap();

	Ok(())
}
