use std::path::PathBuf;

use smithay::backend::input::*;

pub struct TestInputBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TestDevice;

impl Device for TestDevice {
    fn id(&self) -> String {
        "test-input".into()
    }
    fn name(&self) -> String {
        "Test Input".into()
    }
    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(
            capability,
            DeviceCapability::Pointer | DeviceCapability::Touch
        )
    }
    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }
    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

impl InputBackend for TestInputBackend {
    type Device = TestDevice;
    type KeyboardKeyEvent = UnusedEvent;
    type PointerAxisEvent = TestEvent;
    type PointerButtonEvent = TestEvent;
    type PointerMotionEvent = TestEvent;
    type PointerMotionAbsoluteEvent = TestEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = TestEvent;
    type TouchUpEvent = TestEvent;
    type TouchMotionEvent = TestEvent;
    type TouchCancelEvent = TestEvent;
    type TouchFrameEvent = TestEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TestEvent {
    pub x: f64,
    pub y: f64,
    pub pressed: bool,
    pub slot: u32,
}

impl Event<TestInputBackend> for TestEvent {
    fn time(&self) -> u64 {
        1000
    }
    fn device(&self) -> TestDevice {
        TestDevice
    }
}

impl AbsolutePositionEvent<TestInputBackend> for TestEvent {
    fn x(&self) -> f64 {
        self.x
    }
    fn y(&self) -> f64 {
        self.y
    }
    fn x_transformed(&self, width: i32) -> f64 {
        self.x * f64::from(width)
    }
    fn y_transformed(&self, height: i32) -> f64 {
        self.y * f64::from(height)
    }
}

impl PointerMotionAbsoluteEvent<TestInputBackend> for TestEvent {}

impl PointerMotionEvent<TestInputBackend> for TestEvent {
    fn delta_x(&self) -> f64 {
        self.x
    }
    fn delta_y(&self) -> f64 {
        self.y
    }
    fn delta_x_unaccel(&self) -> f64 {
        self.x
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.y
    }
}

impl PointerButtonEvent<TestInputBackend> for TestEvent {
    fn button_code(&self) -> u32 {
        0x110
    }
    fn state(&self) -> ButtonState {
        if self.pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        }
    }
}

impl PointerAxisEvent<TestInputBackend> for TestEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        Some(match axis {
            Axis::Horizontal => self.x,
            Axis::Vertical => self.y,
        })
    }
    fn amount_v120(&self, _: Axis) -> Option<f64> {
        None
    }
    fn source(&self) -> AxisSource {
        AxisSource::Finger
    }
    fn relative_direction(&self, _: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

impl TouchEvent<TestInputBackend> for TestEvent {
    fn slot(&self) -> TouchSlot {
        Some(self.slot).into()
    }
}

impl TouchDownEvent<TestInputBackend> for TestEvent {}
impl TouchMotionEvent<TestInputBackend> for TestEvent {}
impl TouchUpEvent<TestInputBackend> for TestEvent {}
impl TouchCancelEvent<TestInputBackend> for TestEvent {}
impl TouchFrameEvent<TestInputBackend> for TestEvent {}

pub fn position(x: f64, y: f64) -> TestEvent {
    TestEvent {
        x,
        y,
        ..TestEvent::default()
    }
}
