#![allow(dead_code)]

use std::time::Instant;

use integer_sqrt::IntegerSquareRoot;

const MAX_MOVES: usize = 4;
const DEFAULT_MIN_DISTANCE: u32 = 200;

#[derive(Debug, Default, Clone, Copy)]
pub struct Location {
	pub x: u32,
	pub y: u32,
}

impl Location {
	fn new(x: u32, y: u32) -> Self {
		Self { x, y }
	}
}

impl Location {
	fn dist(self, loc: Location) -> u32 {
		let x_d = self.x.abs_diff(loc.x).pow(2);
		let y_d = self.y.abs_diff(loc.y).pow(2);
		(x_d + y_d).integer_sqrt()
	}
}

impl From<&winit::dpi::PhysicalPosition<f64>> for Location {
	fn from(value: &winit::dpi::PhysicalPosition<f64>) -> Self {
		Self {
			x: value.x.round() as u32,
			y: value.y.round() as u32,
		}
	}
}

#[derive(Debug)]
enum Phase {
	Started(Instant),
	Ended(Instant, Instant),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Direction {
	UpRight,
	Up,
	UpLeft,
	Left,
	DownLeft,
	Down,
	DownRight,
	Right,
}

impl Direction {
	// Calculate move in 2d space where positive y is down
	fn into_move(a: Location, b: Location) -> Direction {
		use std::f32::consts;
		let x_d = b.x as f32 - a.x as f32;
		let y_d = b.y as f32 - a.y as f32;
		let r = consts::PI + y_d.atan2(x_d);
		let bucket = (r * (8.0 / consts::PI)).round() as u32;
		match bucket {
			0 => Direction::Left,
			1..=2 => Direction::UpLeft,
			3..=4 => Direction::Up,
			5..=6 => Direction::UpRight,
			7..=8 => Direction::Right,
			9..=10 => Direction::DownRight,
			11..=12 => Direction::Down,
			13..=14 => Direction::DownLeft,
			15..=16 => Direction::Left,
			_ => panic!("Ehm, shouldn't be possible?"),
		}
	}
}

#[derive(Debug)]
struct GestureState {
	id: u64,
	ph: Phase,
	loc: Location,
	idx: usize,
	moves: [Option<(Direction, u8)>; MAX_MOVES],
}

impl GestureState {
	fn active_id(&self, finger_id: u64) -> bool {
		matches!(self.ph, Phase::Started(_)) && self.id == finger_id
	}

	fn record_direction(&mut self, l: Location) {
		// Check that we have space
		if self.idx >= MAX_MOVES - 2 {
			return;
		}
		// Determine direction and set loc
		let m = Direction::into_move(self.loc, l);
		self.loc = l;

		// Update or add move
		let id = self.id;
		let idx = self.idx;
		if let Some((mv, d)) = self.moves[idx] {
			if mv == m {
				log::trace!("Move same {m:?} for {id}");
				self.moves[idx] = Some((mv, d + 1));
			} else {
				log::trace!("Move new {m:?} for {id}");
				self.moves[idx + 1] = Some((m, 1));
				self.idx += 1;
			}
		} else {
			log::trace!("Move first {m:?} for {id}");
			self.moves[idx] = Some((m, 1));
		}
	}
}

pub struct GestureTracker<const F: usize> {
	min_distance: u32,
	cursor_loc: Location,
	states: [Option<GestureState>; F],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
	Tap,
	Swipe(Direction, u8),
	Swipe2(Direction, Direction, u8),
	Swipe3(Direction, Direction, Direction, u8),
	Swipe4(Direction, Direction, Direction, Direction, u8),
}

impl Gesture {
	fn from_moves(moves: &[Option<(Direction, u8)>; MAX_MOVES]) -> Option<Gesture> {
		match moves {
			[None, None, None, None] => Some(Gesture::Tap),
			[Some(m1), None, None, None] => Some(Gesture::Swipe(m1.0, m1.1)),
			[Some(m1), Some(m2), None, None] => Some(Gesture::Swipe2(m1.0, m2.0, m1.1 + m2.1)),
			[Some(m1), Some(m2), Some(m3), None] => {
				Some(Gesture::Swipe3(m1.0, m2.0, m3.0, m1.1 + m2.1 + m3.1))
			}
			[Some(m1), Some(m2), Some(m3), Some(m4)] => Some(Gesture::Swipe4(
				m1.0,
				m2.0,
				m3.0,
				m4.0,
				m1.1 + m2.1 + m3.1 + m4.1,
			)),
			_ => None,
		}
	}
}

#[derive(Debug, Clone)]
pub struct GestureEvent {
	pub start: Instant,
	pub end: Instant,
	pub loc: Location,
	pub gesture: Gesture,
}

#[derive(Clone)]
pub struct GestureIter<'a> {
	idx: usize,
	states: &'a [Option<GestureState>],
}

impl<'a> Iterator for GestureIter<'a> {
	type Item = GestureEvent;

	fn next(&mut self) -> Option<Self::Item> {
		for i in self.idx..self.states.len() {
			if let Some(state) = &self.states[i]
				&& let Phase::Ended(start, end) = state.ph
				&& let Some(gesture) = Gesture::from_moves(&state.moves)
			{
				let loc = state.loc;
				self.idx = i + 1;
				return Some(GestureEvent {
					start,
					end,
					loc,
					gesture,
				});
			}
		}
		None
	}
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum GestureTrackerStatus {
	Idle,
	Tracking,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum GestureTrackerResult {
	Ignored,
	Captured,
}

impl GestureTrackerStatus {
	pub fn idle(self) -> bool {
		matches!(self, GestureTrackerStatus::Idle)
	}
}

impl<const F: usize> GestureTracker<F> {
	pub fn new() -> Self {
		Self {
			min_distance: DEFAULT_MIN_DISTANCE,
			cursor_loc: Location::default(),
			states: [const { None }; F],
		}
	}

	pub fn events<'a>(&'a self) -> GestureIter<'a> {
		GestureIter {
			idx: 0,
			states: &self.states,
		}
	}

	pub fn set_min_distance(&mut self, min_distance: u32) {
		self.min_distance = min_distance;
	}

	pub fn set_min_distance_by_screen(&mut self, width: u32, height: u32) {
		self.min_distance = width
			.min(height)
			.checked_div(12)
			.unwrap_or(DEFAULT_MIN_DISTANCE);
	}

	pub fn reset(&mut self) {
		self.states = [const { None }; F];
	}

	pub fn status(&self) -> GestureTrackerStatus {
		if self
			.states
			.iter()
			.all(|s| s.as_ref().is_none_or(|s| matches!(s.ph, Phase::Ended(..))))
		{
			GestureTrackerStatus::Idle
		} else {
			GestureTrackerStatus::Tracking
		}
	}

	pub fn touch_start(&mut self, finger_id: u64, l: impl Into<Location>) -> GestureTrackerResult {
		if let Some(state) = self.states.iter_mut().find(|s| s.is_none()) {
			log::trace!("Starting touch {finger_id}");
			let t = Instant::now();
			*state = Some(GestureState {
				id: finger_id,
				ph: Phase::Started(t),
				loc: l.into(),
				idx: 0,
				moves: [const { None }; MAX_MOVES],
			});
			GestureTrackerResult::Captured
		} else {
			GestureTrackerResult::Ignored
		}
	}

	pub fn touch_move(&mut self, finger_id: u64, l: impl Into<Location>) -> GestureTrackerResult {
		let state = self
			.states
			.iter_mut()
			.find(|s| s.as_ref().is_some_and(|s| s.active_id(finger_id)));
		if let Some(Some(state)) = state {
			let l = l.into();
			let d = state.loc.dist(l);
			if d > self.min_distance {
				state.record_direction(l);
			}
			GestureTrackerResult::Captured
		} else {
			GestureTrackerResult::Ignored
		}
	}

	pub fn touch_end(&mut self, finger_id: u64, l: impl Into<Location>) -> GestureTrackerResult {
		let state = self
			.states
			.iter_mut()
			.find(|s| s.as_ref().is_some_and(|s| s.active_id(finger_id)));
		if let Some(Some(state)) = state {
			let l = l.into();
			let d = state.loc.dist(l);
			if d > self.min_distance {
				state.record_direction(l);
			}
			if let Phase::Started(t0) = state.ph {
				log::trace!("Ending touch {finger_id}");
				let t = Instant::now();
				state.ph = Phase::Ended(t0, t);
				state.loc = l;
			} else {
				log::warn!("Ending already ended state, id {finger_id}");
			}
			GestureTrackerResult::Captured
		} else {
			GestureTrackerResult::Ignored
		}
	}

	pub fn touch_cancel(&mut self, finger_id: u64) -> GestureTrackerResult {
		let state = self
			.states
			.iter_mut()
			.find(|s| s.as_ref().is_some_and(|s| s.active_id(finger_id)));
		if let Some(state) = state {
			log::info!("cancel touch {}", finger_id);
			*state = None;
			GestureTrackerResult::Captured
		} else {
			GestureTrackerResult::Ignored
		}
	}

	pub fn handle_window_event(&mut self, event: &winit::event::WindowEvent) -> EventResult {
		use winit::event::ElementState;
		use winit::event::Touch;
		use winit::event::TouchPhase;
		use winit::event::WindowEvent;

		match event {
			WindowEvent::CursorMoved { position, .. } => {
				self.cursor_loc = position.into();
				let result = self.touch_move(0, self.cursor_loc);
				EventResult {
					frame_ended: false,
					consumed: matches!(result, GestureTrackerResult::Captured),
				}
			}
			WindowEvent::MouseInput { state, .. } => {
				let (result, frame_ended) = match state {
					ElementState::Pressed => (self.touch_start(0, self.cursor_loc), false),
					ElementState::Released => {
						let result = self.touch_end(0, self.cursor_loc);
						let frame_ended = matches!(result, GestureTrackerResult::Captured)
							&& matches!(self.status(), GestureTrackerStatus::Idle);
						(result, frame_ended)
					}
				};
				EventResult {
					frame_ended,
					consumed: matches!(result, GestureTrackerResult::Captured),
				}
			}
			WindowEvent::Touch(Touch {
				id,
				location,
				phase,
				..
			}) => {
				let (result, frame_ended) = match phase {
					TouchPhase::Started => (self.touch_start(*id, location), false),
					TouchPhase::Moved => (self.touch_move(*id, location), false),
					TouchPhase::Ended => {
						let result = self.touch_end(*id, location);
						let frame_ended = matches!(result, GestureTrackerResult::Captured)
							&& matches!(self.status(), GestureTrackerStatus::Idle);
						(result, frame_ended)
					}
					TouchPhase::Cancelled => {
						let result = self.touch_cancel(*id);
						let frame_ended = matches!(result, GestureTrackerResult::Captured)
							&& matches!(self.status(), GestureTrackerStatus::Idle);
						(result, frame_ended)
					}
				};
				EventResult {
					frame_ended,
					consumed: matches!(result, GestureTrackerResult::Captured),
				}
			}
			_ => EventResult {
				frame_ended: false,
				consumed: false,
			},
		}
	}
}

#[derive(Debug)]
pub struct EventResult {
	pub frame_ended: bool,
	pub consumed: bool,
}

#[cfg(test)]
mod tests {
	use crate::gestures::Gesture;
	use crate::gestures::GestureTracker;
	use crate::gestures::GestureTrackerResult;
	use crate::gestures::GestureTrackerStatus;

	use super::Direction;
	use super::Location as L;

	#[test]
	fn test_move_basic() {
		println!("Right");
		assert_eq!(
			Direction::Right,
			Direction::into_move(L::new(0, 0), L::new(1, 0))
		);
		println!("Left");
		assert_eq!(
			Direction::Left,
			Direction::into_move(L::new(1, 0), L::new(0, 0))
		);
		println!("Down");
		assert_eq!(
			Direction::Down,
			Direction::into_move(L::new(0, 0), L::new(0, 1))
		);
		println!("Up");
		assert_eq!(
			Direction::Up,
			Direction::into_move(L::new(0, 1), L::new(0, 0))
		);
		println!("DownRight");
		assert_eq!(
			Direction::DownRight,
			Direction::into_move(L::new(0, 0), L::new(1, 1))
		);
		println!("DownLeft");
		assert_eq!(
			Direction::DownLeft,
			Direction::into_move(L::new(1, 0), L::new(0, 1))
		);
		println!("UpRight");
		assert_eq!(
			Direction::UpRight,
			Direction::into_move(L::new(0, 1), L::new(1, 0))
		);
		println!("UpLeft");
		assert_eq!(
			Direction::UpLeft,
			Direction::into_move(L::new(1, 1), L::new(0, 0))
		);
	}

	#[test]
	fn test_tracker_basic() {
		let mut tracker = GestureTracker::<1>::new();
		tracker.set_min_distance(0);

		let l = L::new(0, 0); // origin
		tracker.touch_start(0, l);
		for i in 1..10 {
			let l = L::new(i, 0); // moving right
			tracker.touch_move(0, l);
		}
		let l = L::new(10, 0);
		assert_eq!(GestureTrackerResult::Captured, tracker.touch_end(0, l));
		assert_eq!(GestureTrackerStatus::Idle, tracker.status());

		let gs = tracker.events();
		let gs = gs.collect::<Vec<_>>();
		assert_eq!(1, gs.len());
		let g = &gs[0];
		assert_eq!(Gesture::Swipe(Direction::Right, 10), g.gesture);
	}
}
