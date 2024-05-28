use std::ffi::NulError;

use anyhow::{anyhow, Context};
use sdl2::{
    controller::{Axis, Button, GameController},
    joystick::Guid,
    EventPump, GameControllerSubsystem, JoystickSubsystem, Sdl,
};
use vexide_simulator_protocol::{ControllerState, ControllerUpdate};
use wasmtime::*;

use crate::sdk::SdkState;

use super::JumpTableBuilder;

// MARK: Constants

/// `vex-sdk` excerpt.
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

            let controller = caller
                .data_mut()
                .inputs
                .controller(id, false)
                .context("Invalid controller id")?;
            if let Some(controller) = controller {
                let states = controller.current_state;
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
            } else {
                Ok(0)
            }
        },
    );

    // vexControllerConnectionStatusGet
    builder.insert(
        0x1a8,
        move |mut caller: Caller<'_, SdkState>, id: u32| -> Result<i32> {
            caller.data_mut().inputs.connected(id).map(|c| c as i32)
        },
    );
}

// MARK: API

pub struct V5Controller {
    pub current_state: ControllerState,
    pub sdl_guid: Option<Guid>,
    pub sdl_controller: Option<GameController>,
}

pub struct Inputs {
    controllers: [Option<V5Controller>; 2],
    controller_subsystem: GameControllerSubsystem,
    joystick_subsystem: JoystickSubsystem,
    event_pump: EventPump,
}

impl Inputs {
    pub fn new(sdl: Sdl) -> Self {
        Inputs {
            controllers: Default::default(),
            event_pump: sdl.event_pump().unwrap(),
            joystick_subsystem: sdl.joystick().unwrap(),
            controller_subsystem: sdl.game_controller().unwrap(),
        }
    }

    pub fn set_controller(
        &mut self,
        id: u32,
        update: Option<ControllerUpdate>,
    ) -> Result<(), Error> {
        assert!(
            id < self.controllers.len() as u32,
            "Invalid controller index"
        );

        match update {
            Some(update) => {
                let controller = match update {
                    ControllerUpdate::Raw(state) => V5Controller {
                        current_state: state,
                        sdl_controller: None,
                        sdl_guid: None,
                    },
                    ControllerUpdate::UUID(uuid) => V5Controller {
                        // TODO: use Default::default()
                        current_state: ControllerState {
                            axis1: 0,
                            axis2: 0,
                            axis3: 0,
                            axis4: 0,
                            button_l1: false,
                            button_l2: false,
                            button_r1: false,
                            button_r2: false,
                            button_up: false,
                            button_down: false,
                            button_left: false,
                            button_right: false,
                            button_x: false,
                            button_b: false,
                            button_y: false,
                            button_a: false,
                            button_sel: false,
                            battery_level: 0,
                            button_all: false,
                            flags: 0,
                            battery_capacity: 0,
                        },
                        sdl_controller: None,
                        sdl_guid: Some(Guid::from_string(&uuid)?),
                    },
                };
                self.controllers[id as usize] = Some(controller);
            }
            None => {
                self.controllers[id as usize] = None;
            }
        }

        Ok(())
    }

    /// Returns whether the controller with the given id is connected.
    ///
    /// Fails if the id is invalid.
    pub fn connected(&mut self, id: u32) -> Result<bool> {
        Ok(self.controller(id, false)?.is_some())
    }

    // /// Find a suitable game controller for the given index.
    // fn find_suitable_controller_id(&self, mut id: usize) -> Option<GameController> {
    //     for index in 0..self.subsystem.num_joysticks().ok()? {
    //         if self.subsystem.is_game_controller(index) {
    //             let Ok(controller) = self.subsystem.open(index) else {
    //                 continue;
    //             };
    //             if !controller.attached() {
    //                 continue;
    //             }
    //             if id == 0 {
    //                 return Some(controller);
    //             } else {
    //                 id -= 1;
    //             }
    //         }
    //     }
    //     None
    // }

    /// Get the connected controller for the given index, or try to connect a new one if it is not connected.
    pub fn controller(&mut self, id: u32, discover: bool) -> Result<Option<&mut V5Controller>> {
        if id >= self.controllers.len() as u32 {
            anyhow::bail!("Invalid controller id")
        }

        let Some(controller) = self.controllers[id as usize].as_mut() else {
            return Ok(None);
        };
        if let Some(guid) = &controller.sdl_guid {
            // If the frontend provided a controller ID, the sim controller is only connected if the physical controller it refers to is connected.
            if let Some(sdl_controller) = &mut controller.sdl_controller {
                if sdl_controller.attached() {
                    return Ok(Some(controller));
                } else {
                    controller.sdl_controller = None;
                }
            }

            // At this point the SDL controller isn't valid so we try and discover one with that GUID.
            if discover {
                let joysticks = self
                    .controller_subsystem
                    .num_joysticks()
                    .map_err(|s| anyhow!(s))?;
                for idx in 0..joysticks {
                    if self.controller_subsystem.is_game_controller(idx) {
                        let Ok(joystick) = self.joystick_subsystem.open(idx) else {
                            continue;
                        };
                        if &joystick.guid() != guid || !joystick.attached() {
                            continue;
                        }
                        let Ok(sdl_controller) = self.controller_subsystem.open(idx) else {
                            continue;
                        };

                        controller.sdl_controller = Some(sdl_controller);
                        return Ok(Some(controller));
                    }
                }
            }

            Ok(None)
        } else {
            // The frontend didn't provide a controller ID for updating it so we're just left with a constant controller state.
            Ok(Some(controller))
        }
    }

    /// Get new events from the SDL event pump and update the SDK's representation of the controller states.
    pub fn update(&mut self) -> anyhow::Result<()> {
        self.event_pump.pump_events();

        for index in 0..self.controllers.len() {
            if let Some(controller) = self.controller(index as u32, true)? {
                if let Some(sdl_controller) = &controller.sdl_controller {
                    controller.current_state.axis1 =
                        (sdl_controller.axis(Axis::LeftX) as i32) * 127 / (i16::MAX as i32);
                    controller.current_state.axis2 =
                        -(sdl_controller.axis(Axis::LeftY) as i32) * 127 / (i16::MAX as i32);
                    controller.current_state.axis3 =
                        -(sdl_controller.axis(Axis::RightY) as i32) * 127 / (i16::MAX as i32);
                    controller.current_state.axis4 =
                        (sdl_controller.axis(Axis::RightX) as i32) * 127 / (i16::MAX as i32);
                    controller.current_state.button_l1 =
                        sdl_controller.button(Button::LeftShoulder);
                    controller.current_state.button_l2 = sdl_controller.axis(Axis::TriggerLeft) > 0;
                    controller.current_state.button_r1 =
                        sdl_controller.button(Button::RightShoulder);
                    controller.current_state.button_r2 =
                        sdl_controller.axis(Axis::TriggerRight) > 0;
                    controller.current_state.button_up = sdl_controller.button(Button::DPadUp);
                    controller.current_state.button_down = sdl_controller.button(Button::DPadDown);
                    controller.current_state.button_left = sdl_controller.button(Button::DPadLeft);
                    controller.current_state.button_right =
                        sdl_controller.button(Button::DPadRight);
                    controller.current_state.button_x = sdl_controller.button(Button::X);
                    controller.current_state.button_b = sdl_controller.button(Button::B);
                    controller.current_state.button_y = sdl_controller.button(Button::Y);
                    controller.current_state.button_a = sdl_controller.button(Button::A);
                }
                // self.states[index].button_sel = controller.button(Button::Start);
                // self.states[index].battery_level =
                // self.states[index].button_all =
                // self.states[index].flags =
                // self.states[index].battery_capacity =
            }
        }

        Ok(())
    }
}
