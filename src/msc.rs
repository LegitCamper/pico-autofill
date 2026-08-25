use embassy_futures::select::{Either, select};
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::peripherals::FLASH;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::{Driver, Endpoint, EndpointError, EndpointIn, EndpointOut};
use embassy_usb::{Builder, Handler};

use pico_autofill::fat::{DISK_BYTES, SECTOR_COUNT, SECTOR_SIZE, extract_text, format_disk};
use pico_autofill::persist;
use pico_autofill::text::AutofillText;
use portable_atomic::Ordering;

use crate::{
    CURRENT_TEXT, FLASH_DATA_OFFSET, FLASH_SECTOR_SIZE, MEDIA_PRESENT, RESET_REQUEST,
    STORAGE_COMMAND, StorageCommand, TYPE_REQUESTS,
};

const MSC_CLASS: u8 = 0x08;
const SCSI_TRANSPARENT_SUBCLASS: u8 = 0x06;
const BULK_ONLY_PROTOCOL: u8 = 0x50;
const BULK_PACKET_SIZE: u16 = 64;

const CBW_SIGNATURE: u32 = 0x4342_5355;
const CSW_SIGNATURE: u32 = 0x5342_5355;

pub struct MscClass<'d, D: Driver<'d>> {
    out: D::EndpointOut,
    input: D::EndpointIn,
}

impl<'d, D: Driver<'d>> MscClass<'d, D> {
    pub fn new(builder: &mut Builder<'d, D>, control: &'d mut MscControl) -> Self {
        let mut function =
            builder.function(MSC_CLASS, SCSI_TRANSPARENT_SUBCLASS, BULK_ONLY_PROTOCOL);
        let mut interface = function.interface();
        let interface_number = interface.interface_number();
        let mut alt = interface.alt_setting(
            MSC_CLASS,
            SCSI_TRANSPARENT_SUBCLASS,
            BULK_ONLY_PROTOCOL,
            None,
        );
        let out = alt.endpoint_bulk_out(None, BULK_PACKET_SIZE);
        let input = alt.endpoint_bulk_in(None, BULK_PACKET_SIZE);
        drop(function);

        control.interface = u8::from(interface_number);
        builder.handler(control);
        Self { out, input }
    }

    pub async fn run(
        mut self,
        disk: &mut [u8; DISK_BYTES],
        flash: &mut Flash<'_, FLASH, Blocking, { 2 * 1024 * 1024 }>,
        mut current_text: AutofillText,
    ) -> ! {
        let mut sense = Sense::no_sense();
        let mut dirty = false;
        let mut unit_attention = false;
        let mut cbw_bytes = [0_u8; 31];

        loop {
            if RESET_REQUEST.swap(false, Ordering::AcqRel) {
                sense = Sense::no_sense();
            }

            let event = select(self.out.read(&mut cbw_bytes), STORAGE_COMMAND.wait()).await;
            let read_len = match event {
                Either::First(Ok(len)) => len,
                Either::First(Err(EndpointError::Disabled)) => {
                    self.out.wait_enabled().await;
                    continue;
                }
                Either::First(Err(EndpointError::BufferOverflow)) => continue,
                Either::Second(command) => {
                    self.handle_storage_command(
                        command,
                        disk,
                        flash,
                        &mut current_text,
                        &mut dirty,
                        &mut unit_attention,
                    )
                    .await;
                    continue;
                }
            };

            if read_len != cbw_bytes.len() {
                continue;
            }
            let Some(cbw) = Cbw::parse(&cbw_bytes) else {
                continue;
            };

            let result = self
                .serve_scsi_command(&cbw, disk, &mut sense, &mut dirty, &mut unit_attention)
                .await;
            let post_action = match result {
                Ok(action) => action,
                Err(EndpointError::Disabled) => {
                    self.out.wait_enabled().await;
                    continue;
                }
                Err(EndpointError::BufferOverflow) => continue,
            };

            match post_action {
                PostAction::None => {}
                PostAction::Sync => {
                    commit_disk(disk, flash, &mut current_text, &mut dirty).await;
                }
                PostAction::Eject => {
                    let saved = commit_disk(disk, flash, &mut current_text, &mut dirty).await;
                    MEDIA_PRESENT.store(false, Ordering::Release);
                    if saved {
                        format_disk(disk, &current_text);
                    }
                }
            }
        }
    }

    async fn handle_storage_command(
        &mut self,
        command: StorageCommand,
        disk: &mut [u8; DISK_BYTES],
        flash: &mut Flash<'_, FLASH, Blocking, { 2 * 1024 * 1024 }>,
        current_text: &mut AutofillText,
        dirty: &mut bool,
        unit_attention: &mut bool,
    ) {
        match command {
            StorageCommand::Insert => {
                if !*dirty {
                    format_disk(disk, current_text);
                }
                *unit_attention = true;
                MEDIA_PRESENT.store(true, Ordering::Release);
            }
            StorageCommand::EjectAndType => {
                let saved = commit_disk(disk, flash, current_text, dirty).await;
                MEDIA_PRESENT.store(false, Ordering::Release);
                if saved {
                    format_disk(disk, current_text);
                }
                TYPE_REQUESTS.send(()).await;
            }
        }
    }

    async fn serve_scsi_command(
        &mut self,
        cbw: &Cbw,
        disk: &mut [u8; DISK_BYTES],
        sense: &mut Sense,
        dirty: &mut bool,
        unit_attention: &mut bool,
    ) -> Result<PostAction, EndpointError> {
        let media_present = MEDIA_PRESENT.load(Ordering::Acquire);
        let opcode = cbw.cdb[0];
        let mut post_action = PostAction::None;
        let mut transferred = 0_u32;
        let success = match opcode {
            0x00 => {
                if !media_present {
                    *sense = Sense::medium_not_present();
                    false
                } else if *unit_attention {
                    *unit_attention = false;
                    *sense = Sense::medium_changed();
                    false
                } else {
                    true
                }
            }
            0x03 => {
                let response = sense.response();
                transferred = self.send_data(cbw, &response).await?;
                *sense = Sense::no_sense();
                true
            }
            0x12 => {
                let response = inquiry_response(cbw.cdb[1] & 1 != 0, cbw.cdb[2]);
                if response.is_supported() {
                    transferred = self.send_data(cbw, response.as_slice()).await?;
                    true
                } else {
                    *sense = Sense::invalid_field();
                    false
                }
            }
            0x1a => {
                transferred = self.send_data(cbw, &[3, 0, 0, 0]).await?;
                true
            }
            0x1b => {
                if cbw.cdb[4] & 0x02 != 0 && cbw.cdb[4] & 0x01 == 0 {
                    post_action = PostAction::Eject;
                }
                true
            }
            0x1e | 0x2f => true,
            0x23 => {
                if !media_present {
                    *sense = Sense::medium_not_present();
                    false
                } else {
                    let mut response = [0_u8; 12];
                    response[3] = 8;
                    response[4..8].copy_from_slice(&(SECTOR_COUNT as u32).to_be_bytes());
                    response[8] = 0x02;
                    response[9..12].copy_from_slice(&[0, 2, 0]);
                    transferred = self.send_data(cbw, &response).await?;
                    true
                }
            }
            0x25 => {
                if !media_present {
                    *sense = Sense::medium_not_present();
                    false
                } else {
                    let mut response = [0_u8; 8];
                    response[..4].copy_from_slice(&((SECTOR_COUNT - 1) as u32).to_be_bytes());
                    response[4..].copy_from_slice(&(SECTOR_SIZE as u32).to_be_bytes());
                    transferred = self.send_data(cbw, &response).await?;
                    true
                }
            }
            0x28 => {
                let (offset, len) = read_write_range(&cbw.cdb);
                if !media_present {
                    *sense = Sense::medium_not_present();
                    false
                } else if let Some(range) = checked_disk_range(offset, len) {
                    transferred = self.send_data(cbw, &disk[range]).await?;
                    true
                } else {
                    *sense = Sense::lba_out_of_range();
                    false
                }
            }
            0x2a => {
                let (offset, len) = read_write_range(&cbw.cdb);
                if !media_present {
                    *sense = Sense::medium_not_present();
                    false
                } else if let Some(range) = checked_disk_range(offset, len) {
                    transferred = self.receive_data(cbw, &mut disk[range]).await?;
                    let written = transferred == cbw.transfer_len && transferred as usize == len;
                    *dirty |= written;
                    written
                } else {
                    *sense = Sense::lba_out_of_range();
                    false
                }
            }
            0x35 => {
                if media_present {
                    post_action = PostAction::Sync;
                    true
                } else {
                    *sense = Sense::medium_not_present();
                    false
                }
            }
            0x5a => {
                transferred = self.send_data(cbw, &[0, 6, 0, 0, 0, 0, 0, 0]).await?;
                true
            }
            0x9e if cbw.cdb[1] & 0x1f == 0x10 => {
                if !media_present {
                    *sense = Sense::medium_not_present();
                    false
                } else {
                    let mut response = [0_u8; 32];
                    response[..8].copy_from_slice(&((SECTOR_COUNT - 1) as u64).to_be_bytes());
                    response[8..12].copy_from_slice(&(SECTOR_SIZE as u32).to_be_bytes());
                    transferred = self.send_data(cbw, &response).await?;
                    true
                }
            }
            0xa0 => {
                let mut response = [0_u8; 16];
                response[3] = 8;
                transferred = self.send_data(cbw, &response).await?;
                true
            }
            _ => {
                *sense = Sense::invalid_command();
                false
            }
        };

        if !success {
            self.finish_failed_data_phase(cbw, transferred).await?;
        }
        let residue = if success {
            cbw.transfer_len.saturating_sub(transferred)
        } else {
            0
        };
        self.send_csw(cbw.tag, if success { 0 } else { 1 }, residue)
            .await?;
        Ok(post_action)
    }

    async fn send_data(&mut self, cbw: &Cbw, data: &[u8]) -> Result<u32, EndpointError> {
        let length = data.len().min(cbw.transfer_len as usize);
        for chunk in data[..length].chunks(BULK_PACKET_SIZE as usize) {
            self.input.write(chunk).await?;
        }
        Ok(length as u32)
    }

    async fn receive_data(
        &mut self,
        cbw: &Cbw,
        destination: &mut [u8],
    ) -> Result<u32, EndpointError> {
        let expected = cbw.transfer_len as usize;
        let mut received = 0;
        let mut packet = [0_u8; BULK_PACKET_SIZE as usize];
        while received < expected {
            let request = (expected - received).min(packet.len());
            let count = self.out.read(&mut packet[..request]).await?;
            if count == 0 {
                break;
            }
            if received < destination.len() {
                let copy_len = count.min(destination.len() - received);
                destination[received..received + copy_len].copy_from_slice(&packet[..copy_len]);
            }
            received += count;
        }
        Ok(received as u32)
    }

    async fn finish_failed_data_phase(
        &mut self,
        cbw: &Cbw,
        transferred: u32,
    ) -> Result<(), EndpointError> {
        let mut remaining = cbw.transfer_len.saturating_sub(transferred) as usize;
        let mut scratch = [0_u8; BULK_PACKET_SIZE as usize];
        if cbw.device_to_host() {
            while remaining != 0 {
                let count = remaining.min(scratch.len());
                self.input.write(&scratch[..count]).await?;
                remaining -= count;
            }
        } else {
            while remaining != 0 {
                let request = remaining.min(scratch.len());
                let count = self.out.read(&mut scratch[..request]).await?;
                if count == 0 {
                    break;
                }
                remaining -= count;
            }
        }
        Ok(())
    }

    async fn send_csw(&mut self, tag: u32, status: u8, residue: u32) -> Result<(), EndpointError> {
        let mut csw = [0_u8; 13];
        csw[..4].copy_from_slice(&CSW_SIGNATURE.to_le_bytes());
        csw[4..8].copy_from_slice(&tag.to_le_bytes());
        csw[8..12].copy_from_slice(&residue.to_le_bytes());
        csw[12] = status;
        self.input.write(&csw).await
    }
}

pub struct MscControl {
    interface: u8,
}

impl MscControl {
    pub const fn new() -> Self {
        Self { interface: 0xff }
    }

    fn matches(&self, req: Request) -> bool {
        req.request_type == RequestType::Class
            && req.recipient == Recipient::Interface
            && req.index == u16::from(self.interface)
    }
}

impl Handler for MscControl {
    fn control_out(&mut self, req: Request, _data: &[u8]) -> Option<OutResponse> {
        if !self.matches(req) {
            return None;
        }
        if req.request == 0xff && req.value == 0 && req.length == 0 {
            RESET_REQUEST.store(true, Ordering::Release);
            Some(OutResponse::Accepted)
        } else {
            Some(OutResponse::Rejected)
        }
    }

    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if !self.matches(req) {
            return None;
        }
        if req.request == 0xfe && req.value == 0 && req.length == 1 && !buf.is_empty() {
            buf[0] = 0;
            Some(InResponse::Accepted(&buf[..1]))
        } else {
            Some(InResponse::Rejected)
        }
    }
}

struct Cbw {
    tag: u32,
    transfer_len: u32,
    flags: u8,
    cdb: [u8; 16],
}

impl Cbw {
    fn parse(bytes: &[u8; 31]) -> Option<Self> {
        let signature = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let cdb_len = bytes[14] & 0x1f;
        if signature != CBW_SIGNATURE || !(1..=16).contains(&cdb_len) || bytes[13] != 0 {
            return None;
        }
        let mut cdb = [0_u8; 16];
        cdb.copy_from_slice(&bytes[15..31]);
        Some(Self {
            tag: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            transfer_len: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            flags: bytes[12],
            cdb,
        })
    }

    const fn device_to_host(&self) -> bool {
        self.flags & 0x80 != 0
    }
}

#[derive(Clone, Copy)]
struct Sense {
    key: u8,
    asc: u8,
    ascq: u8,
}

impl Sense {
    const fn no_sense() -> Self {
        Self {
            key: 0,
            asc: 0,
            ascq: 0,
        }
    }

    const fn medium_not_present() -> Self {
        Self {
            key: 0x02,
            asc: 0x3a,
            ascq: 0,
        }
    }

    const fn medium_changed() -> Self {
        Self {
            key: 0x06,
            asc: 0x28,
            ascq: 0,
        }
    }

    const fn invalid_command() -> Self {
        Self {
            key: 0x05,
            asc: 0x20,
            ascq: 0,
        }
    }

    const fn invalid_field() -> Self {
        Self {
            key: 0x05,
            asc: 0x24,
            ascq: 0,
        }
    }

    const fn lba_out_of_range() -> Self {
        Self {
            key: 0x05,
            asc: 0x21,
            ascq: 0,
        }
    }

    fn response(self) -> [u8; 18] {
        let mut response = [0_u8; 18];
        response[0] = 0x70;
        response[2] = self.key;
        response[7] = 10;
        response[12] = self.asc;
        response[13] = self.ascq;
        response
    }
}

enum InquiryResponse {
    Standard([u8; 36]),
    SupportedPages([u8; 7]),
    Serial([u8; 12]),
    DeviceId([u8; 16]),
    Unsupported([u8; 4]),
}

impl InquiryResponse {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Standard(value) => value,
            Self::SupportedPages(value) => value,
            Self::Serial(value) => value,
            Self::DeviceId(value) => value,
            Self::Unsupported(value) => value,
        }
    }

    const fn is_supported(&self) -> bool {
        !matches!(self, Self::Unsupported(_))
    }
}

fn inquiry_response(evpd: bool, page: u8) -> InquiryResponse {
    if !evpd {
        let mut response = [0_u8; 36];
        response[0] = 0;
        response[1] = 0x80;
        response[2] = 0x04;
        response[3] = 0x02;
        response[4] = 31;
        response[8..16].copy_from_slice(b"RP2040  ");
        response[16..32].copy_from_slice(b"AUTOFILL DISK   ");
        response[32..36].copy_from_slice(b"1.00");
        return InquiryResponse::Standard(response);
    }

    match page {
        0x00 => InquiryResponse::SupportedPages([0, 0, 0, 3, 0, 0x80, 0x83]),
        0x80 => InquiryResponse::Serial([
            0, 0x80, 0, 8, b'R', b'P', b'2', b'0', b'4', b'0', b'A', b'F',
        ]),
        0x83 => InquiryResponse::DeviceId([
            0, 0x83, 0, 12, 0x02, 0x01, 0, 8, b'R', b'P', b'2', b'0', b'4', b'0', b'A', b'F',
        ]),
        _ => InquiryResponse::Unsupported([0, page, 0, 0]),
    }
}

#[derive(Clone, Copy)]
enum PostAction {
    None,
    Sync,
    Eject,
}

fn read_write_range(cdb: &[u8; 16]) -> (usize, usize) {
    let lba = u32::from_be_bytes([cdb[2], cdb[3], cdb[4], cdb[5]]) as usize;
    let blocks = u16::from_be_bytes([cdb[7], cdb[8]]) as usize;
    (
        lba.saturating_mul(SECTOR_SIZE),
        blocks.saturating_mul(SECTOR_SIZE),
    )
}

fn checked_disk_range(offset: usize, len: usize) -> Option<core::ops::Range<usize>> {
    let end = offset.checked_add(len)?;
    (end <= DISK_BYTES).then_some(offset..end)
}

async fn commit_disk(
    disk: &[u8; DISK_BYTES],
    flash: &mut Flash<'_, FLASH, Blocking, { 2 * 1024 * 1024 }>,
    current_text: &mut AutofillText,
    dirty: &mut bool,
) -> bool {
    if !*dirty {
        return true;
    }
    let Some(updated) = extract_text(disk) else {
        return false;
    };
    let record = persist::encode(&updated);
    let erase_end = FLASH_DATA_OFFSET + FLASH_SECTOR_SIZE;
    let saved = flash.blocking_erase(FLASH_DATA_OFFSET, erase_end).is_ok()
        && flash.blocking_write(FLASH_DATA_OFFSET, &record).is_ok();

    *current_text = updated.clone();
    CURRENT_TEXT.lock(|text| *text.borrow_mut() = updated);
    if saved {
        *dirty = false;
    }
    saved
}
