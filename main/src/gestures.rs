#![allow(dead_code)]

use std::time::Duration;
use std::time::Instant;

use integer_sqrt::IntegerSquareRoot;

const MAX_MOVES: usize = 10;
const DEFAULT_MIN_DISTANCE: u32 = 200;

#[derive(Debug, Default, Clone, Copy)]
pub struct Location {
	x: u32,
	y: u32,
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

impl From<winit::dpi::PhysicalPosition<f64>> for Location {
	fn from(value: winit::dpi::PhysicalPosition<f64>) -> Self {
		Self {
			x: value.x.round() as u32,
			y: value.y.round() as u32,
		}
	}
}

#[derive(Debug)]
enum Phase {
	Started(Instant),
	Ended(Duration),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Move {
	UpRight,
	Up,
	UpLeft,
	Left,
	DownLeft,
	Down,
	DownRight,
	Right,
}

impl Move {
	// Calculate move in 2d space where positive y is down
	fn into_move(a: Location, b: Location) -> Move {
		use std::f32::consts;
		let x_d = b.x as f32 - a.x as f32;
		let y_d = b.y as f32 - a.y as f32;
		let r = consts::PI + y_d.atan2(x_d);
		let bucket = (r * (8.0 / consts::PI)).round() as u32;
		match bucket {
			0 => Move::Left,
			1..=2 => Move::UpLeft,
			3..=4 => Move::Up,
			5..=6 => Move::UpRight,
			7..=8 => Move::Right,
			9..=10 => Move::DownRight,
			11..=12 => Move::Down,
			13..=14 => Move::DownLeft,
			15..=16 => Move::Left,
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
	moves: [Option<(Move, u8)>; MAX_MOVES],
}

impl GestureState {
	fn active_id(&self, finger_id: u64) -> bool {
		matches!(self.ph, Phase::Started(_)) && self.id == finger_id
	}
}

pub struct GestureTracker<const F: usize> {
	min_distance: u32,
	states: [Option<GestureState>; F],
}

#[derive(Clone)]
pub struct GestureMoveIter<'a> {
	idx: usize,
	d: Duration,
	moves: &'a [Option<(Move, u8)>; MAX_MOVES],
}

impl GestureMoveIter<'_> {
	fn duration(&self) -> Duration {
		self.d
	}
}

impl<'a> Iterator for GestureMoveIter<'a> {
	type Item = (Move, u8);

	fn next(&mut self) -> Option<Self::Item> {
		if let Some(m) = self.moves[self.idx] {
			self.idx += 1;
			Some(m)
		} else {
			None
		}
	}
}

#[derive(Clone)]
pub struct GestureIter<'a, const F: usize> {
	idx: usize,
	states: &'a [Option<GestureState>; F],
}

impl<const F: usize> GestureIter<'_, F> {
	fn into_vec(self) -> Vec<(Duration, Vec<(Move, u8)>)> {
		self.map(|ms| (ms.d, ms.collect::<Vec<_>>())).collect::<Vec<_>>()
	}
}

impl<'a, const F: usize> Iterator for GestureIter<'a, F> {
	type Item = GestureMoveIter<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		for i in self.idx..self.states.len() {
			if let Some(state) = &self.states[i]
				&& let Phase::Ended(d) = state.ph
			{
				self.idx = i + 1;
				return Some(GestureMoveIter {
					idx: 0,
					d,
					moves: &state.moves,
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

impl GestureTrackerStatus {
	pub fn idle(self) -> bool {
		matches!(self, GestureTrackerStatus::Idle)
	}
}

impl<const F: usize> GestureTracker<F> {
	pub fn new() -> Self {
		Self {
			min_distance: DEFAULT_MIN_DISTANCE,
			states: [const { None }; F],
		}
	}

	pub fn gestures<'a>(&'a self) -> GestureIter<'a, F> {
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
			.checked_div(10)
			.unwrap_or(DEFAULT_MIN_DISTANCE);
	}

	pub fn reset(&mut self) {
		log::info!("Reset");
		self.states = [const { None }; F];
	}

	pub fn touch_start(&mut self, finger_id: u64, l: impl Into<Location>) {
		if let Some(state) = self.states.iter_mut().find(|s| s.is_none()) {
			log::info!("Starting touch {finger_id}");
			let t = Instant::now();
			*state = Some(GestureState {
				id: finger_id,
				ph: Phase::Started(t),
				loc: l.into(),
				idx: 0,
				moves: [const { None }; MAX_MOVES],
			});
		}
	}

	pub fn touch_move(&mut self, finger_id: u64, l: impl Into<Location>) {
		let state = self
			.states
			.iter_mut()
			.find(|s| s.as_ref().is_some_and(|s| s.active_id(finger_id)));
		if let Some(Some(state)) = state {
			let l = l.into();
			let d = state.loc.dist(l);
			if d > self.min_distance {
				record_direction(state, l);
			}
		}
	}

	pub fn touch_end(&mut self, finger_id: u64, l: impl Into<Location>) -> GestureTrackerStatus {
		let state = self
			.states
			.iter_mut()
			.find(|s| s.as_ref().is_some_and(|s| s.active_id(finger_id)));
		if let Some(Some(state)) = state {
			let l = l.into();
			let d = state.loc.dist(l);
			if d > self.min_distance {
				record_direction(state, l);
			}
			if let Phase::Started(t0) = state.ph {
				log::info!("Ending touch {finger_id}");
				let t = Instant::now();
				let d = t.duration_since(t0);
				state.ph = Phase::Ended(d);
			} else {
				log::warn!("Ending already ended state, id {finger_id}");
			}
		}
		self.status()
	}

	pub fn touch_cancel(&mut self, finger_id: u64) -> GestureTrackerStatus {
		let state = self
			.states
			.iter_mut()
			.find(|s| s.as_ref().is_some_and(|s| s.active_id(finger_id)));
		if let Some(state) = state {
			log::info!("cancel touch {}", finger_id);
			*state = None;
		}
		self.status()
	}

	fn status(&self) -> GestureTrackerStatus {
		if self
			.states
			.iter()
			.all(|s| s.as_ref().is_none_or(|s| matches!(s.ph, Phase::Ended(_))))
		{
			GestureTrackerStatus::Idle
		} else {
			GestureTrackerStatus::Tracking
		}
	}
}

fn record_direction(state: &mut GestureState, l: Location) {
	// Check that we have space
	if state.idx >= MAX_MOVES - 2 {
		return;
	}
	// Determine direction and set loc
	let m = Move::into_move(state.loc, l);
	state.loc = l;

	// Update or add move
	let id = state.id;
	let idx = state.idx;
	if let Some((mv, d)) = state.moves[idx] {
		if mv == m {
			log::info!("Move same {m:?} for {id}");
			state.moves[idx] = Some((mv, d + 1));
		} else {
			log::info!("Move new {m:?} for {id}");
			state.moves[idx + 1] = Some((m, 1));
			state.idx += 1;
		}
	} else {
		log::info!("Move first {m:?} for {id}");
		state.moves[idx] = Some((m, 1));
	}
}

#[cfg(test)]
mod tests {
	use crate::gestures::GestureTracker;

	use super::Location as L;
	use super::Move;

	#[test]
	fn test_move_basic() {
		println!("Right");
		assert_eq!(Move::Right, Move::into_move(L::new(0, 0), L::new(1, 0)));
		println!("Left");
		assert_eq!(Move::Left, Move::into_move(L::new(1, 0), L::new(0, 0)));
		println!("Down");
		assert_eq!(Move::Down, Move::into_move(L::new(0, 0), L::new(0, 1)));
		println!("Up");
		assert_eq!(Move::Up, Move::into_move(L::new(0, 1), L::new(0, 0)));
		println!("DownRight");
		assert_eq!(Move::DownRight, Move::into_move(L::new(0, 0), L::new(1, 1)));
		println!("DownLeft");
		assert_eq!(Move::DownLeft, Move::into_move(L::new(1, 0), L::new(0, 1)));
		println!("UpRight");
		assert_eq!(Move::UpRight, Move::into_move(L::new(0, 1), L::new(1, 0)));
		println!("UpLeft");
		assert_eq!(Move::UpLeft, Move::into_move(L::new(1, 1), L::new(0, 0)));
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
		assert!(tracker.touch_end(0, l).idle());

		let gs = tracker.gestures();
		let gs = gs.into_vec();
		assert_eq!(1, gs.len());
		let g = &gs[0];
		assert_eq!(1, g.1.len());
		assert_eq!(10, g.1[0].1);
		assert_eq!(Move::Right, g.1[0].0);
	}
}
