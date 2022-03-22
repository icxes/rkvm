use crate::event::Button;
use crate::event::Event;
use crate::event::KeyKind;
use crate::linux::device_id;
use crate::linux::glue::{self, input_event, libevdev, libevdev_uinput};
use std::io::{Error, ErrorKind};
use std::mem::MaybeUninit;
use std::ops::RangeInclusive;

struct Device {
    evdev: *mut libevdev,
    uinput: *mut libevdev_uinput,
}

impl Device {
    pub async fn new(event_types: &'static [(u32, &[RangeInclusive<u32>])]) -> Result<Self, Error> {
        tokio::task::spawn_blocking(move || Self::new_sync(event_types)).await?
    }

    fn new_sync(event_types: &'static [(u32, &[RangeInclusive<u32>])]) -> Result<Self, Error> {
        let evdev = unsafe { glue::libevdev_new() };
        if evdev.is_null() {
            return Err(Error::new(ErrorKind::Other, "Failed to create device"));
        }

        if let Err(err) = unsafe { setup_evdev(evdev, event_types) } {
            unsafe {
                glue::libevdev_free(evdev);
            }

            return Err(err);
        }

        let mut uinput = MaybeUninit::uninit();
        let ret = unsafe {
            glue::libevdev_uinput_create_from_device(
                evdev,
                glue::libevdev_uinput_open_mode_LIBEVDEV_UINPUT_OPEN_MANAGED,
                uinput.as_mut_ptr(),
            )
        };

        if ret < 0 {
            unsafe { glue::libevdev_free(evdev) };
            return Err(Error::from_raw_os_error(-ret));
        }

        let uinput = unsafe { uinput.assume_init() };
        Ok(Self { evdev, uinput })
    }

    pub async fn write(&mut self, event: Event) -> Result<(), Error> {
        self.write_raw(event.to_raw())
    }

    fn write_raw(&mut self, event: input_event) -> Result<(), Error> {
        // As far as tokio is concerned, the FD never becomes ready for writing, so just write it normally.
        // If an error happens, it will be propagated to caller and the FD is opened in nonblocking mode anyway,
        // so it shouldn't be an issue.
        let events = [
            (event.type_, event.code, event.value),
            (glue::EV_SYN as _, glue::SYN_REPORT as _, 0), // Include EV_SYN.
        ];

        for (r#type, code, value) in events.iter().cloned() {
            let ret = unsafe {
                glue::libevdev_uinput_write_event(
                    self.uinput as *const _,
                    r#type as _,
                    code as _,
                    value,
                )
            };

            if ret < 0 {
                return Err(Error::from_raw_os_error(-ret));
            }
        }

        Ok(())
    }
}

unsafe fn setup_evdev(
    evdev: *mut libevdev,
    event_types: &'static [(u32, &[RangeInclusive<u32>])],
) -> Result<(), Error> {
    glue::libevdev_set_name(evdev, b"rkvm\0".as_ptr() as *const _);
    glue::libevdev_set_id_vendor(evdev, device_id::VENDOR as _);
    glue::libevdev_set_id_product(evdev, device_id::PRODUCT as _);
    glue::libevdev_set_id_version(evdev, device_id::VERSION as _);
    glue::libevdev_set_id_bustype(evdev, glue::BUS_USB as _);

    for (r#type, codes) in event_types.iter().copied() {
        let ret = glue::libevdev_enable_event_type(evdev, r#type);
        if ret < 0 {
            return Err(Error::from_raw_os_error(-ret));
        }

        for code in codes.iter().cloned().flatten() {
            let ret = glue::libevdev_enable_event_code(evdev, r#type, code, std::ptr::null_mut());
            if ret < 0 {
                return Err(Error::from_raw_os_error(-ret));
            }
        }
    }

    Ok(())
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            glue::libevdev_uinput_destroy(self.uinput);
            glue::libevdev_free(self.evdev);
        }
    }
}

unsafe impl Send for Device {}

struct Mouse {
    device: Device,
}

const MOUSE_EVENT_TYPES: &[(u32, &[RangeInclusive<u32>])] = &[
    (glue::EV_SYN, &[glue::SYN_REPORT..=glue::SYN_REPORT]),
    (glue::EV_REL, &[0..=glue::REL_MAX]),
    (glue::EV_KEY, &[glue::BTN_LEFT..=glue::BTN_GEAR_UP]),
];

impl Mouse {
    pub async fn new() -> Result<Self, Error> {
        let device = Device::new(MOUSE_EVENT_TYPES).await?;
        Ok(Self { device })
    }

    pub async fn write(&mut self, event: Event) -> Result<(), Error> {
        self.device.write(event).await?;
        Ok(())
    }
}

struct Keyboard {
    device: Device,
}

const KEYBOARD_EVENT_TYPES: &[(u32, &[RangeInclusive<u32>])] = &[
    (glue::EV_SYN, &[glue::SYN_REPORT..=glue::SYN_REPORT]),
    (glue::EV_KEY, &[0..=glue::KEY_MICMUTE]),
];

impl Keyboard {
    pub async fn new() -> Result<Self, Error> {
        let device = Device::new(KEYBOARD_EVENT_TYPES).await?;
        Ok(Self { device })
    }

    pub async fn write(&mut self, event: Event) -> Result<(), Error> {
        self.device.write(event).await?;
        Ok(())
    }
}

pub struct EventWriter {
    mouse: Mouse,
    keyboard: Keyboard,
}

impl EventWriter {
    pub async fn new() -> Result<Self, Error> {
        let mouse = Mouse::new().await?;
        let keyboard = Keyboard::new().await?;
        Ok(Self { mouse, keyboard })
    }

    pub async fn write(&mut self, event: Event) -> Result<(), Error> {
        match event {
            Event::MouseScroll { delta } => self.mouse.write(Event::MouseScroll { delta }).await,
            Event::MouseMove { axis, delta } => {
                self.mouse.write(Event::MouseMove { axis, delta }).await
            }
            Event::Key { direction, kind } => match kind {
                KeyKind::Button(Button::Left)
                | KeyKind::Button(Button::Right)
                | KeyKind::Button(Button::Middle) => {
                    self.mouse.write(Event::Key { direction, kind }).await
                }
                _ => self.keyboard.write(Event::Key { direction, kind }).await,
            },
        }
    }
}
