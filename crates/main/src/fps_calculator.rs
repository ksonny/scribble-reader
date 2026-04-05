use std::time::Instant;

pub(crate) struct FpsCalculator {
	last_frame: Instant,
	total_ms: u64,
}

impl FpsCalculator {
	const DIVIDER_2: u64 = 3;

	pub(crate) fn new() -> Self {
		Self {
			last_frame: Instant::now(),
			total_ms: 0,
		}
	}

	pub(crate) fn tick(&mut self) {
		let instant = Instant::now();
		let frame = instant.duration_since(self.last_frame).as_millis() as u64;
		let avg = self.total_ms >> Self::DIVIDER_2;
		self.total_ms = self.total_ms + frame - avg;
		self.last_frame = instant;
	}

	#[allow(dead_code)]
	pub(crate) fn fps(&self) -> u64 {
		(1000_u64 << Self::DIVIDER_2)
			.checked_div(self.total_ms)
			.unwrap_or(0)
	}
}
