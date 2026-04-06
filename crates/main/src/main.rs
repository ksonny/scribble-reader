use std::sync::Arc;

use scribble_reader::Paths;
use scribble_reader::start;
use scribe::config::ScribeConfig;
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
		cache_path: xdg_dirs.get_cache_home().unwrap(),
		config_path: xdg_dirs.get_config_home().unwrap(),
		data_path: xdg_dirs.get_data_home().unwrap(),
	};

	// TODO: Figuring out library path should be moved to wrangler
	let config = ScribeConfig::load(paths.config_path.as_path())?;
	let lib_path = config
		.library
		.path
		.as_ref()
		.map(|p| p.as_str())
		.unwrap_or("~/Documents/ebook/")
		.to_string();
	let (system, handle) = create_wrangler(DocumentTree::new(Arc::new(lib_path)));

	let event_loop = EventLoop::with_user_event().build().unwrap();

	start(paths, system.clone(), event_loop)?;

	system.send(wrangler::WranglerCommand::Shutdown);
	handle.join().unwrap();

	Ok(())
}
