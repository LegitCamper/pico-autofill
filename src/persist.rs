use crate::text::{AutofillText, MAX_TEXT_LEN};

pub const FLASH_RECORD_SIZE: usize = 256;
const MAGIC: &[u8; 4] = b"PAF1";
const VERSION: u8 = 1;
const DATA_OFFSET: usize = 8;
const CRC_OFFSET: usize = DATA_OFFSET + MAX_TEXT_LEN;

pub fn encode(text: &AutofillText) -> [u8; FLASH_RECORD_SIZE] {
    let mut record = [0xff_u8; FLASH_RECORD_SIZE];
    record[..4].copy_from_slice(MAGIC);
    record[4] = VERSION;
    record[5] = text.len() as u8;
    record[6] = 0;
    record[7] = 0;
    record[DATA_OFFSET..DATA_OFFSET + text.len()].copy_from_slice(text.as_bytes());
    let crc = crc32(&record[..CRC_OFFSET]);
    record[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    record
}

pub fn decode(record: &[u8; FLASH_RECORD_SIZE]) -> Option<AutofillText> {
    if &record[..4] != MAGIC || record[4] != VERSION || usize::from(record[5]) > MAX_TEXT_LEN {
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
    let len = usize::from(record[5]);
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
