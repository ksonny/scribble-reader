use crate::gestures;

#[derive(Debug, Clone, Copy)]
pub enum ActiveAreaAction {
	ToggleUi,
	NextPage,
	PreviousPage,
}

#[derive(Debug)]
pub struct ActiveArea {
	action: ActiveAreaAction,
	x_min: u32,
	x_max: u32,
	y_min: u32,
	y_max: u32,
}

impl ActiveArea {
	pub fn contains(&self, pos: gestures::Location) -> bool {
		pos.x >= self.x_min && pos.x < self.x_max && pos.y >= self.y_min && pos.y < self.y_max
	}
}

#[derive(Debug)]
pub struct ActiveAreas([ActiveArea; 3]);

impl Default for ActiveAreas {
	fn default() -> Self {
		Self::new(0, 0)
	}
}

impl ActiveAreas {
	pub fn new(width: u32, height: u32) -> Self {
		let forth = width / 4;
		Self([
			ActiveArea {
				action: ActiveAreaAction::PreviousPage,
				x_min: 0,
				x_max: forth,
				y_min: 0,
				y_max: height,
			},
			ActiveArea {
				action: ActiveAreaAction::ToggleUi,
				x_min: forth,
				x_max: 3 * forth,
				y_min: 0,
				y_max: height,
			},
			ActiveArea {
				action: ActiveAreaAction::NextPage,
				x_min: 3 * forth,
				x_max: width,
				y_min: 0,
				y_max: height,
			},
		])
	}

	pub fn action(&self, pos: gestures::Location) -> Option<ActiveAreaAction> {
		let ActiveAreas(areas) = self;
		areas.iter().find(|a| a.contains(pos)).map(|a| a.action)
	}
}
