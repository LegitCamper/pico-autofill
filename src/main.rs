#![no_std]
#![no_main]

mod msc;

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_futures::join::join4;
use embassy_rp::bind_interrupts;
use embassy_rp::bootsel::is_bootsel_pressed;
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::peripherals::{BOOTSEL, USB};
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use embassy_usb::class::hid::{
    Config as HidConfig, HidBootProtocol, HidProtocolMode, HidSubclass, HidWriter, ReportId,
    RequestHandler, State as HidState,
};
use embassy_usb::control::OutResponse;
use embassy_usb::driver::{Driver as UsbDriver, EndpointError};
use embassy_usb::{Builder, Config};
use panic_halt as _;
use pico_autofill::click::{ClickAction, ClickDetector};
use pico_autofill::fat::{DISK_BYTES, format_disk};
use pico_autofill::keyboard::{ascii_to_hid, needs_intermediate_release};
use pico_autofill::persist::{self, FLASH_RECORD_SIZE};
use pico_autofill::text::AutofillText;
use portable_atomic::{AtomicBool, AtomicU8, Ordering};
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

use crate::msc::{MscClass, MscControl};

const FLASH_SIZE: usize = 2 * 1024 * 1024;
pub(crate) const FLASH_DATA_OFFSET: u32 = 0x001f_f000;
pub(crate) const FLASH_SECTOR_SIZE: u32 = 4096;
const _: () = assert!(FLASH_RECORD_SIZE <= FLASH_SECTOR_SIZE as usize);

pub(crate) static MEDIA_PRESENT: AtomicBool = AtomicBool::new(false);
pub(crate) static RESET_REQUEST: AtomicBool = AtomicBool::new(false);
static HID_PROTOCOL: AtomicU8 = AtomicU8::new(HidProtocolMode::Report as u8);

pub(crate) static STORAGE_COMMAND: Signal<CriticalSectionRawMutex, StorageCommand> = Signal::new();
pub(crate) static TYPE_REQUESTS: Channel<CriticalSectionRawMutex, (), 2> = Channel::new();
pub(crate) static CURRENT_TEXT: Mutex<CriticalSectionRawMutex, RefCell<AutofillText>> =
    Mutex::new(RefCell::new(AutofillText::empty()));

#[derive(Clone, Copy)]
pub(crate) enum StorageCommand {
    Insert,
    EjectAndType,
}

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let mut flash = Flash::<_, Blocking, FLASH_SIZE>::new_blocking(p.FLASH);
    let current_text = load_saved_text(&mut flash);
    CURRENT_TEXT.lock(|text| *text.borrow_mut() = current_text.clone());
    let mut disk = [0_u8; DISK_BYTES];
    format_disk(&mut disk, &current_text);

    let driver = Driver::new(p.USB, Irqs);
    let mut usb_config = Config::new(0x1209, 0x2040);
    usb_config.manufacturer = Some("RP2040 Autofill");
    usb_config.product = Some("Autofill Keyboard and Disk");
    usb_config.serial_number = Some("PICOAF01");
    usb_config.max_power = 100;

    let mut config_descriptor = [0_u8; 256];
    let mut bos_descriptor = [0_u8; 64];
    let mut msos_descriptor = [0_u8; 64];
    let mut control_buffer = [0_u8; 64];
    let mut hid_state = HidState::new();
    let mut hid_handler = KeyboardRequestHandler;
    let mut msc_control = MscControl::new();
    let mut builder = Builder::new(
        driver,
        usb_config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buffer,
    );

    let hid_config = HidConfig {
        report_descriptor: KeyboardReport::desc(),
        request_handler: Some(&mut hid_handler),
        poll_ms: 1,
        max_packet_size: 8,
        hid_subclass: HidSubclass::Boot,
        hid_boot_protocol: HidBootProtocol::Keyboard,
    };
    let hid_writer = HidWriter::<_, 8>::new(&mut builder, &mut hid_state, hid_config);

    let msc = MscClass::new(&mut builder, &mut msc_control);
    let mut usb = builder.build();

    join4(
        usb.run(),
        keyboard_loop(hid_writer),
        button_loop(p.BOOTSEL),
        msc.run(&mut disk, &mut flash, current_text),
    )
    .await;
}

fn load_saved_text(
    flash: &mut Flash<'_, embassy_rp::peripherals::FLASH, Blocking, FLASH_SIZE>,
) -> AutofillText {
    let mut record = [0_u8; FLASH_RECORD_SIZE];
    if flash.blocking_read(FLASH_DATA_OFFSET, &mut record).is_err() {
        return AutofillText::empty();
    }
    match persist::decode(&record) {
        Some(text) => text,
        None => AutofillText::empty(),
    }
}

async fn button_loop(mut bootsel: embassy_rp::Peri<'static, BOOTSEL>) -> ! {
    let now = Instant::now().as_millis();
    let initially_pressed = is_bootsel_pressed(bootsel.reborrow());
    let mut detector = ClickDetector::new(now, initially_pressed);

    loop {
        Timer::after_millis(10).await;
        let pressed = is_bootsel_pressed(bootsel.reborrow());
        let Some(action) = detector.update(pressed, Instant::now().as_millis()) else {
            continue;
        };
        match action {
            ClickAction::MountStorage => STORAGE_COMMAND.signal(StorageCommand::Insert),
            ClickAction::TypeText => {
                if MEDIA_PRESENT.load(Ordering::Acquire) {
                    STORAGE_COMMAND.signal(StorageCommand::EjectAndType);
                } else {
                    TYPE_REQUESTS.send(()).await;
                }
            }
        }
    }
}

async fn keyboard_loop<'d, D: UsbDriver<'d>>(mut writer: HidWriter<'d, D, 8>) -> ! {
    loop {
        TYPE_REQUESTS.receive().await;
        let current_text = CURRENT_TEXT.lock(|text| text.borrow().clone());
        let mut previous_keycode = None;
        for &byte in current_text.as_bytes() {
            let Some((modifier, keycode)) = ascii_to_hid(byte) else {
                continue;
            };
            if needs_intermediate_release(previous_keycode, keycode) {
                write_report(&mut writer, [0; 8]).await;
            }
            write_report(&mut writer, [modifier, 0, keycode, 0, 0, 0, 0, 0]).await;
            previous_keycode = Some(keycode);
        }
        if previous_keycode.is_some() {
            write_report(&mut writer, [0; 8]).await;
        }
    }
}

async fn write_report<'d, D: UsbDriver<'d>>(writer: &mut HidWriter<'d, D, 8>, report: [u8; 8]) {
    loop {
        match writer.write(&report).await {
            Ok(()) => return,
            Err(EndpointError::Disabled) => writer.ready().await,
            Err(EndpointError::BufferOverflow) => return,
        }
    }
}

struct KeyboardRequestHandler;

impl RequestHandler for KeyboardRequestHandler {
    fn set_report(&mut self, _id: ReportId, _data: &[u8]) -> OutResponse {
        OutResponse::Accepted
    }

    fn get_protocol(&self) -> HidProtocolMode {
        HidProtocolMode::from(HID_PROTOCOL.load(Ordering::Relaxed))
    }

    fn set_protocol(&mut self, protocol: HidProtocolMode) -> OutResponse {
        HID_PROTOCOL.store(protocol as u8, Ordering::Relaxed);
        OutResponse::Accepted
    }
}
