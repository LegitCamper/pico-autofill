use crate::text::{AutofillText, MAX_TEXT_LEN};

pub const FLASH_RECORD_SIZE: usize = 2_560;
const MAGIC: &[u8; 4] = b"PAF1";
const VERSION: u8 = 2;
const DATA_OFFSET: usize = 8;
const CRC_OFFSET: usize = DATA_OFFSET + MAX_TEXT_LEN;
const VERSION_1_DATA_LEN: usize = 50;
const VERSION_1_CRC_OFFSET: usize = DATA_OFFSET + VERSION_1_DATA_LEN;
const _: () = assert!(CRC_OFFSET + core::mem::size_of::<u32>() <= FLASH_RECORD_SIZE);
const _: () = assert!(FLASH_RECORD_SIZE.is_multiple_of(256));

pub fn encode(text: &AutofillText) -> [u8; FLASH_RECORD_SIZE] {
    let mut record = [0xff_u8; FLASH_RECORD_SIZE];
    record[..4].copy_from_slice(MAGIC);
    record[4] = VERSION;
    record[5..7].copy_from_slice(&(text.len() as u16).to_le_bytes());
    record[7] = 0;
    record[DATA_OFFSET..DATA_OFFSET + text.len()].copy_from_slice(text.as_bytes());
    let crc = crc32(&record[..CRC_OFFSET]);
    record[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    record
}

pub fn decode(record: &[u8; FLASH_RECORD_SIZE]) -> Option<AutofillText> {
    if &record[..4] != MAGIC {
        return None;
    }
    if record[4] == 1 {
        return decode_version_1(record);
    }
    if record[4] != VERSION {
        return None;
    }

    let len = usize::from(u16::from_le_bytes([record[5], record[6]]));
    if len > MAX_TEXT_LEN {
        return None;
    }
    let stored_crc = u32::from_le_bytes([
        record[CRC_OFFSET],
        record[CRC_OFFSET + 1],
        record[CRC_OFFSET + 2],
        record[CRC_OFFSET + 3],
    ]);
    if crc32(&record[..CRC_OFFSET]) != stored_crc {
        return None;
    }
    Some(AutofillText::from_file_bytes(
        &record[DATA_OFFSET..DATA_OFFSET + len],
    ))
}

fn decode_version_1(record: &[u8; FLASH_RECORD_SIZE]) -> Option<AutofillText> {
    let len = usize::from(record[5]);
    if len > VERSION_1_DATA_LEN {
        return None;
    }
    let stored_crc = u32::from_le_bytes([
        record[VERSION_1_CRC_OFFSET],
        record[VERSION_1_CRC_OFFSET + 1],
        record[VERSION_1_CRC_OFFSET + 2],
        record[VERSION_1_CRC_OFFSET + 3],
    ]);
    if crc32(&record[..VERSION_1_CRC_OFFSET]) != stored_crc {
        return None;
    }
    Some(AutofillText::from_file_bytes(
        &record[DATA_OFFSET..DATA_OFFSET + len],
    ))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_record_should_round_trip() {
        let expected = AutofillText::from_file_bytes(b"saved in flash");
        assert_eq!(decode(&encode(&expected)), Some(expected));
    }

    #[test]
    fn flash_record_should_round_trip_full_capacity() {
        let input = [b'x'; MAX_TEXT_LEN];
        let expected = AutofillText::from_file_bytes(&input);
        assert_eq!(decode(&encode(&expected)), Some(expected));
    }

    #[test]
    fn flash_record_should_migrate_original_version() {
        let expected = AutofillText::from_file_bytes(b"old value");
        let mut record = [0xff_u8; FLASH_RECORD_SIZE];
        record[..4].copy_from_slice(MAGIC);
        record[4] = 1;
        record[5] = expected.len() as u8;
        record[6] = 0;
        record[7] = 0;
        record[DATA_OFFSET..DATA_OFFSET + expected.len()].copy_from_slice(expected.as_bytes());
        let crc = crc32(&record[..VERSION_1_CRC_OFFSET]);
        record[VERSION_1_CRC_OFFSET..VERSION_1_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&record), Some(expected));
    }

    #[test]
    fn flash_record_should_reject_corruption() {
        let mut record = encode(&AutofillText::from_file_bytes(b"secret"));
        record[DATA_OFFSET] ^= 1;
        assert_eq!(decode(&record), None);
    }

    #[test]
    fn erased_flash_should_decode_as_empty_state() {
        assert_eq!(decode(&[0xff; FLASH_RECORD_SIZE]), None);
    }
}
