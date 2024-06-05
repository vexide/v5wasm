use std::sync::mpsc;

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
}

pub enum SdlRequest {
    V5Controller {
        guid: Guid,
        response: oneshot::Sender<Result<Option<ControllerState>>>,
    },
    EventPump,
}

pub struct Inputs {
    controllers: [Option<V5Controller>; 2],
    request_channel: mpsc::Sender<SdlRequest>,
}

impl Inputs {
    pub fn new(request_channel: mpsc::Sender<SdlRequest>) -> Self {
        Inputs {
            controllers: Default::default(),
            request_channel,
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
        Ok(self.controller(id, true)?.is_some())
    }

    /// Get the last known state for the given controller.
    ///
    /// If `lazy` is false, the function will communicate with the main thread to get new states for
    /// controllers managed by SDL2.
    pub fn controller(&mut self, id: u32, lazy: bool) -> Result<Option<&mut V5Controller>> {
        if id >= self.controllers.len() as u32 {
            anyhow::bail!("Invalid controller id");
        }

        let Some(controller) = self.controllers[id as usize].as_mut() else {
            return Ok(None);
        };
        if lazy {
            return Ok(Some(controller));
        }
        if let Some(guid) = controller.sdl_guid {
            let (tx, rx) = oneshot::channel();
            let request = SdlRequest::V5Controller { guid, response: tx };
            self.request_channel.send(request).ok();
            let res = rx.recv().map_err(|_| {
                anyhow!("Controller request failed: main thread is not listening")
            })??;

            // If this is None the frontend wants to use a controller even as
            // there is no physical controller connected to the system, so we're
            // left returning a constant controller state.
            if let Some(res) = res {
                controller.current_state = res;
            }
            Ok(Some(controller))
        } else {
            // The frontend didn't provide a controller ID for updating it so we're just left with a constant controller state.
            Ok(Some(controller))
        }
    }

    /// Get new events from the SDL event pump and update the SDK's representation of the controller states.
    pub fn update(&mut self) -> anyhow::Result<()> {
        self.request_channel
            .send(SdlRequest::EventPump)
            .map_err(|_| anyhow!("Event pump request failed: main thread is not listening"))?;

        for index in 0..self.controllers.len() {
            self.controller(index as u32, true)?;
        }

        Ok(())
    }
}
