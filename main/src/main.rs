use scribble_reader::start;
use winit::error::EventLoopError;
use winit::event_loop::EventLoop;

fn main() -> Result<(), EventLoopError> {
	env_logger::builder()
		.filter_level(log::LevelFilter::Info) // Default Log Level
		.parse_default_env()
		.init();
	let event_loop = EventLoop::with_user_event().build().unwrap();
	start(event_loop)?;
	Ok(())
}
