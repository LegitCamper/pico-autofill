use crate::text::{AutofillText, MAX_TEXT_LEN};

pub const SECTOR_SIZE: usize = 512;
pub const SECTOR_COUNT: usize = 64;
pub const DISK_BYTES: usize = SECTOR_SIZE * SECTOR_COUNT;

const FAT_1_SECTOR: usize = 1;
const FAT_2_SECTOR: usize = 2;
const ROOT_DIR_SECTOR: usize = 3;
const ROOT_DIR_SECTORS: usize = 2;
const DATA_START_SECTOR: usize = ROOT_DIR_SECTOR + ROOT_DIR_SECTORS;
const ROOT_ENTRY_COUNT: u16 = 32;
const FILE_NAME: &[u8; 11] = b"AUTOFILLTXT";
const VOLUME_LABEL: &[u8; 11] = b"PICO FILL  ";

pub fn format_disk(disk: &mut [u8; DISK_BYTES], text: &AutofillText) {
    disk.fill(0);
    format_boot_sector(&mut disk[..SECTOR_SIZE]);

    let fat_1 = FAT_1_SECTOR * SECTOR_SIZE;
    let fat_2 = FAT_2_SECTOR * SECTOR_SIZE;
    disk[fat_1] = 0xf8;
    disk[fat_1 + 1] = 0xff;
    disk[fat_1 + 2] = 0xff;
    set_fat12_entry(&mut disk[fat_1..fat_1 + SECTOR_SIZE], 2, 0x0fff);
    let (before_fat_2, at_fat_2) = disk.split_at_mut(fat_2);
    at_fat_2[..SECTOR_SIZE].copy_from_slice(&before_fat_2[fat_1..fat_1 + SECTOR_SIZE]);

    let root = ROOT_DIR_SECTOR * SECTOR_SIZE;
    disk[root..root + 11].copy_from_slice(VOLUME_LABEL);
    disk[root + 11] = 0x08;

    let file_entry = root + 32;
    disk[file_entry..file_entry + 11].copy_from_slice(FILE_NAME);
    disk[file_entry + 11] = 0x20;
    disk[file_entry + 26..file_entry + 28].copy_from_slice(&2_u16.to_le_bytes());
    disk[file_entry + 28..file_entry + 32].copy_from_slice(&(text.len() as u32).to_le_bytes());

    let data = DATA_START_SECTOR * SECTOR_SIZE;
    disk[data..data + text.len()].copy_from_slice(text.as_bytes());
}

pub fn extract_text(disk: &[u8; DISK_BYTES]) -> Option<AutofillText> {
    let root_start = ROOT_DIR_SECTOR * SECTOR_SIZE;
    let root_end = root_start + ROOT_DIR_SECTORS * SECTOR_SIZE;
    let entry = disk[root_start..root_end]
        .chunks_exact(32)
        .take_while(|entry| entry[0] != 0)
        .find(|entry| entry[0] != 0xe5 && entry[11] != 0x0f && &entry[..11] == FILE_NAME)?;

    let file_size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]) as usize;
    if file_size == 0 {
        return Some(AutofillText::empty());
    }

    let mut cluster = u16::from_le_bytes([entry[26], entry[27]]);
    let fat_start = FAT_1_SECTOR * SECTOR_SIZE;
    let fat = &disk[fat_start..fat_start + SECTOR_SIZE];
    let mut raw = [0_u8; MAX_TEXT_LEN];
    let mut copied = 0;
    let mut remaining = file_size;

    for _ in 0..SECTOR_COUNT {
        let sector = cluster_to_sector(cluster)?;
        let take = remaining.min(SECTOR_SIZE).min(MAX_TEXT_LEN - copied);
        let start = sector * SECTOR_SIZE;
        raw[copied..copied + take].copy_from_slice(&disk[start..start + take]);
        copied += take;
        remaining -= remaining.min(SECTOR_SIZE);
        if remaining == 0 || copied == MAX_TEXT_LEN {
            return Some(AutofillText::from_file_bytes(&raw[..copied]));
        }

        let next = read_fat12_entry(fat, cluster)?;
        if next >= 0x0ff8 {
            return Some(AutofillText::from_file_bytes(&raw[..copied]));
        }
        cluster = next;
    }

    None
}

fn format_boot_sector(sector: &mut [u8]) {
    sector[0..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    sector[3..11].copy_from_slice(b"MSDOS5.0");
    sector[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
    sector[13] = 1;
    sector[14..16].copy_from_slice(&1_u16.to_le_bytes());
    sector[16] = 2;
    sector[17..19].copy_from_slice(&ROOT_ENTRY_COUNT.to_le_bytes());
    sector[19..21].copy_from_slice(&(SECTOR_COUNT as u16).to_le_bytes());
    sector[21] = 0xf8;
    sector[22..24].copy_from_slice(&1_u16.to_le_bytes());
    sector[24..26].copy_from_slice(&32_u16.to_le_bytes());
    sector[26..28].copy_from_slice(&64_u16.to_le_bytes());
    sector[36] = 0x80;
    sector[38] = 0x29;
    sector[39..43].copy_from_slice(&0x5049_434f_u32.to_le_bytes());
    sector[43..54].copy_from_slice(VOLUME_LABEL);
    sector[54..62].copy_from_slice(b"FAT12   ");
    sector[510] = 0x55;
    sector[511] = 0xaa;
}

fn cluster_to_sector(cluster: u16) -> Option<usize> {
    let cluster_index = usize::from(cluster.checked_sub(2)?);
    let sector = DATA_START_SECTOR.checked_add(cluster_index)?;
    (sector < SECTOR_COUNT).then_some(sector)
}

fn set_fat12_entry(fat: &mut [u8], cluster: u16, value: u16) {
    let offset = usize::from(cluster) + usize::from(cluster) / 2;
    if cluster & 1 == 0 {
        fat[offset] = value as u8;
        fat[offset + 1] = (fat[offset + 1] & 0xf0) | ((value >> 8) as u8 & 0x0f);
    } else {
        fat[offset] = (fat[offset] & 0x0f) | ((value << 4) as u8 & 0xf0);
        fat[offset + 1] = (value >> 4) as u8;
    }
}

fn read_fat12_entry(fat: &[u8], cluster: u16) -> Option<u16> {
    let offset = usize::from(cluster) + usize::from(cluster) / 2;
    let pair = u16::from_le_bytes([*fat.get(offset)?, *fat.get(offset + 1)?]);
    Some(if cluster & 1 == 0 {
        pair & 0x0fff
    } else {
        pair >> 4
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatted_disk_should_round_trip_text() {
        let expected = AutofillText::from_file_bytes(b"abc123!@#");
        let mut disk = [0_u8; DISK_BYTES];
        format_disk(&mut disk, &expected);
        assert_eq!(extract_text(&disk), Some(expected));
    }

    #[test]
    fn extraction_should_follow_a_relocated_directory_entry() {
        let expected = AutofillText::from_file_bytes(b"relocated");
        let mut disk = [0_u8; DISK_BYTES];
        format_disk(&mut disk, &expected);
        let root = ROOT_DIR_SECTOR * SECTOR_SIZE;
        disk.copy_within(root + 32..root + 64, root + 64);
        disk[root + 32] = 0xe5;
        assert_eq!(extract_text(&disk), Some(expected));
    }

    #[test]
    fn extraction_should_follow_a_reallocated_data_cluster() {
        let expected = AutofillText::from_file_bytes(b"new cluster");
        let mut disk = [0_u8; DISK_BYTES];
        format_disk(&mut disk, &expected);
        let original_data = DATA_START_SECTOR * SECTOR_SIZE;
        let new_data = original_data + SECTOR_SIZE;
        disk.copy_within(original_data..original_data + expected.len(), new_data);

        let root = ROOT_DIR_SECTOR * SECTOR_SIZE;
        disk[root + 32 + 26..root + 32 + 28].copy_from_slice(&3_u16.to_le_bytes());
        let fat = FAT_1_SECTOR * SECTOR_SIZE;
        set_fat12_entry(&mut disk[fat..fat + SECTOR_SIZE], 3, 0x0fff);
        assert_eq!(extract_text(&disk), Some(expected));
    }

    #[test]
    fn extraction_should_reject_a_missing_file() {
        let mut disk = [0_u8; DISK_BYTES];
        format_disk(&mut disk, &AutofillText::empty());
        let root = ROOT_DIR_SECTOR * SECTOR_SIZE;
        disk[root + 32] = 0xe5;
        assert_eq!(extract_text(&disk), None);
    }
}
