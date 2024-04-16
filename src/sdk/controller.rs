use anyhow::Context;
use sdl2::{
    controller::{Axis, Button, GameController},
    EventPump, GameControllerSubsystem,
};
use wasmtime::*;

use crate::sdk::SdkState;

use super::JumpTableBuilder;

// MARK: Constants

mod constants {
    #![allow(non_camel_case_types)]
    #![allow(non_upper_case_globals)]
    #![allow(non_snake_case)]

    #[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
    #[repr(transparent)]
    pub struct V5_ControllerId(pub core::ffi::c_uint);

    impl V5_ControllerId {
        pub const kControllerMaster: Self = Self(0);
        pub const kControllerPartner: Self = Self(1);
    }

    #[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
    #[repr(transparent)]
    pub struct V5_ControllerStatus(pub core::ffi::c_uint);

    impl V5_ControllerStatus {
        pub const kV5ControllerOffline: Self = Self(0);
        pub const kV5ControllerTethered: Self = Self(1);
        pub const kV5ControllerVexnet: Self = Self(2);
    }

    #[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
    #[repr(transparent)]
    pub struct V5_ControllerIndex(pub core::ffi::c_uint);

    impl V5_ControllerIndex {
        pub const AnaLeftX: Self = Self(0);
        pub const AnaLeftY: Self = Self(1);
        pub const AnaRightX: Self = Self(2);
        pub const AnaRightY: Self = Self(3);
        pub const AnaSpare1: Self = Self(4);
        pub const AnaSpare2: Self = Self(5);
        pub const Button5U: Self = Self(6);
        pub const Button5D: Self = Self(7);
        pub const Button6U: Self = Self(8);
        pub const Button6D: Self = Self(9);
        pub const Button7U: Self = Self(10);
        pub const Button7D: Self = Self(11);
        pub const Button7L: Self = Self(12);
        pub const Button7R: Self = Self(13);
        pub const Button8U: Self = Self(14);
        pub const Button8D: Self = Self(15);
        pub const Button8L: Self = Self(16);
        pub const Button8R: Self = Self(17);
        pub const ButtonSEL: Self = Self(18);
        pub const BatteryLevel: Self = Self(19);
        pub const ButtonAll: Self = Self(20);
        pub const Flags: Self = Self(21);
        pub const BatteryCapacity: Self = Self(22);
        pub const Axis1: Self = Self::AnaRightX;
        pub const Axis2: Self = Self::AnaRightY;
        pub const Axis3: Self = Self::AnaLeftY;
        pub const Axis4: Self = Self::AnaLeftX;
        pub const ButtonL1: Self = Self::Button5U;
        pub const ButtonL2: Self = Self::Button5D;
        pub const ButtonR1: Self = Self::Button6U;
        pub const ButtonR2: Self = Self::Button6D;
        pub const ButtonUp: Self = Self::Button7U;
        pub const ButtonDown: Self = Self::Button7D;
        pub const ButtonLeft: Self = Self::Button7L;
        pub const ButtonRight: Self = Self::Button7R;
        pub const ButtonX: Self = Self::Button8U;
        pub const ButtonB: Self = Self::Button8D;
        pub const ButtonY: Self = Self::Button8L;
        pub const ButtonA: Self = Self::Button8R;
    }
}

// MARK: Jump table

pub fn build_controller_jump_table(memory: Memory, builder: &mut JumpTableBuilder) {
    use constants::*;

    // vexControllerGet
    builder.insert(
        0x1a4,
        move |mut caller: Caller<'_, SdkState>, id: u32, index: u32| -> Result<i32> {
            let index = V5_ControllerIndex(index);

            caller.data_mut().inputs.update();
            let states = caller
                .data()
                .inputs
                .states
                .get(id as usize)
                .context("Invalid controller id")?;
            match index {
                V5_ControllerIndex::AnaLeftX => Ok(states.axis1),
                V5_ControllerIndex::AnaLeftY => Ok(states.axis2),
                V5_ControllerIndex::AnaRightX => Ok(states.axis4),
                V5_ControllerIndex::AnaRightY => Ok(states.axis3),
                V5_ControllerIndex::ButtonL1 => Ok(states.button_l1 as i32),
                V5_ControllerIndex::ButtonL2 => Ok(states.button_l2 as i32),
                V5_ControllerIndex::ButtonR1 => Ok(states.button_r1 as i32),
                V5_ControllerIndex::ButtonR2 => Ok(states.button_r2 as i32),
                V5_ControllerIndex::ButtonUp => Ok(states.button_up as i32),
                V5_ControllerIndex::ButtonDown => Ok(states.button_down as i32),
                V5_ControllerIndex::ButtonLeft => Ok(states.button_left as i32),
                V5_ControllerIndex::ButtonRight => Ok(states.button_right as i32),
                V5_ControllerIndex::ButtonX => Ok(states.button_x as i32),
                V5_ControllerIndex::ButtonB => Ok(states.button_b as i32),
                V5_ControllerIndex::ButtonY => Ok(states.button_y as i32),
                V5_ControllerIndex::ButtonA => Ok(states.button_a as i32),
                V5_ControllerIndex::ButtonSEL => Ok(states.button_sel as i32),
                V5_ControllerIndex::BatteryLevel => Ok(states.battery_level),
                V5_ControllerIndex::ButtonAll => Ok(states.button_all as i32),
                V5_ControllerIndex::Flags => Ok(states.flags),
                V5_ControllerIndex::BatteryCapacity => Ok(states.battery_capacity),
                _ => anyhow::bail!("Invalid controller index"),
            }
        },
    );

    // vexControllerConnectionStatusGet
    builder.insert(
        0x1a8,
        move |mut caller: Caller<'_, SdkState>, id: u32| -> Result<i32> {
            caller.data_mut().inputs.update();
            caller.data_mut().inputs.connected(id).map(|c| c as i32)
        },
    );
}

// MARK: API

#[derive(Debug, Default)]
struct ControllerState {
    axis1: i32,
    axis2: i32,
    axis3: i32,
    axis4: i32,
    button_l1: bool,
    button_l2: bool,
    button_r1: bool,
    button_r2: bool,
    button_up: bool,
    button_down: bool,
    button_left: bool,
    button_right: bool,
    button_x: bool,
    button_b: bool,
    button_y: bool,
    button_a: bool,
    button_sel: bool,
    battery_level: i32,
    button_all: bool,
    flags: i32,
    battery_capacity: i32,
}

pub struct Inputs {
    states: [ControllerState; 2],
    subsystem: GameControllerSubsystem,
    event_pump: EventPump,
    game_controllers: [Option<GameController>; 2],
}

impl Inputs {
    pub fn new(subsystem: GameControllerSubsystem) -> Self {
        Inputs {
            states: Default::default(),
            event_pump: subsystem.sdl().event_pump().unwrap(),
            subsystem,
            game_controllers: Default::default(),
        }
    }

    pub fn connected(&mut self, id: u32) -> Result<bool> {
        if id >= self.game_controllers.len() as u32 {
            anyhow::bail!("Invalid controller id")
        }

        Ok(self.controller(id as usize).is_some())
    }

    fn find_suitable_controller_id(&self, mut id: usize) -> Option<GameController> {
        for index in 0..self.subsystem.num_joysticks().ok()? {
            if self.subsystem.is_game_controller(index) {
                let Ok(controller) = self.subsystem.open(index) else {
                    continue;
                };
                if !controller.attached() {
                    continue;
                }
                if index == 0 {
                    return Some(controller);
                } else {
                    id -= 1;
                }
            }
        }
        None
    }

    fn controller(&mut self, index: usize) -> Option<(&mut GameController, &mut ControllerState)> {
        if self.game_controllers[index]
            .as_ref()
            .map_or(true, |c| !c.attached())
        {
            self.game_controllers[index] = self.find_suitable_controller_id(index);
        }
        self.game_controllers[index]
            .as_mut()
            .and_then(|c| c.attached().then_some(c))
            .map(|c| (c, &mut self.states[index]))
    }

    pub fn update(&mut self) {
        self.event_pump.pump_events();

        for index in 0..self.game_controllers.len() {
            let controller = self.controller(index);
            if let Some((controller, state)) = controller {
                state.axis1 = (controller.axis(Axis::LeftX) as i32) * 127 / (i16::MAX as i32);
                state.axis2 = -(controller.axis(Axis::LeftY) as i32) * 127 / (i16::MAX as i32);
                state.axis3 = -(controller.axis(Axis::RightY) as i32) * 127 / (i16::MAX as i32);
                state.axis4 = (controller.axis(Axis::RightX) as i32) * 127 / (i16::MAX as i32);
                state.button_l1 = controller.button(Button::LeftShoulder);
                state.button_l2 = controller.axis(Axis::TriggerLeft) > 0;
                state.button_r1 = controller.button(Button::RightShoulder);
                state.button_r2 = controller.axis(Axis::TriggerRight) > 0;
                state.button_up = controller.button(Button::DPadUp);
                state.button_down = controller.button(Button::DPadDown);
                state.button_left = controller.button(Button::DPadLeft);
                state.button_right = controller.button(Button::DPadRight);
                state.button_x = controller.button(Button::X);
                state.button_b = controller.button(Button::B);
                state.button_y = controller.button(Button::Y);
                state.button_a = controller.button(Button::A);
                // self.states[index].button_sel = controller.button(Button::Start);
                // self.states[index].battery_level =
                // self.states[index].button_all =
                // self.states[index].flags =
                // self.states[index].battery_capacity =
            } else {
                self.states[index] = Default::default();
            }
        }
    }
}
