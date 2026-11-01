//! kbin.rs — writes the KBIN binary format described in KLS v0.2:
//!
//!   Magic         4 bytes   "KBIN"
//!   Version       u32       format version
//!   Entry address u64       absolute VA of the first instruction to run
//!   Code size     u64       bytes
//!   Data size     u64       bytes
//!   Code          [u8; code_size]
//!   Data          [u8; data_size]
//!
//! The image is position-DEPENDENT: it must be loaded/mapped starting at
//! `LOAD_BASE` (see aarch64_codegen.rs) so that absolute addresses baked
//! into the code (string-literal pointers, etc.) resolve correctly.

use alloc::vec::Vec;

pub const KBIN_VERSION: u32 = 1;

pub fn write(entry_address: u64, code: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 4 + 8 + 8 + 8 + code.len() + data.len());
    out.extend_from_slice(b"KBIN");
    out.extend_from_slice(&KBIN_VERSION.to_le_bytes());
    out.extend_from_slice(&entry_address.to_le_bytes());
    out.extend_from_slice(&(code.len() as u64).to_le_bytes());
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(code);
    out.extend_from_slice(data);
    out
}

#[cfg(feature = "test")]
pub mod tests {
    use crate::println;
    use super::*;
    use alloc::vec;

    fn test_magic_bytes() {
        let out = write(0x1000, &[], &[]);
        assert_eq!(&out[0..4], b"KBIN");
    }

    fn test_version_field() {
        let out = write(0x1000, &[], &[]);
        let version = u32::from_le_bytes(out[4..8].try_into().unwrap());
        assert_eq!(version, KBIN_VERSION);
    }

    fn test_entry_address_roundtrip() {
        let out = write(0x5000_0000, &[], &[]);
        let entry = u64::from_le_bytes(out[8..16].try_into().unwrap());
        assert_eq!(entry, 0x5000_0000);
    }

    fn test_code_and_data_size_fields() {
        let code = vec![1, 2, 3, 4];
        let data = vec![5, 6, 7];
        let out = write(0, &code, &data);
        let code_size = u64::from_le_bytes(out[16..24].try_into().unwrap());
        let data_size = u64::from_le_bytes(out[24..32].try_into().unwrap());
        assert_eq!(code_size, 4);
        assert_eq!(data_size, 3);
    }

    fn test_total_length_matches_header_plus_sections() {
        let code = vec![0u8; 40];
        let data = vec![0u8; 17];
        let out = write(0, &code, &data);
        assert_eq!(out.len(), 32 + 40 + 17);
    }

    fn test_code_and_data_bytes_placed_correctly() {
        let code = vec![0xAA, 0xBB];
        let data = vec![0xCC, 0xDD, 0xEE];
        let out = write(0, &code, &data);
        assert_eq!(&out[32..34], &[0xAA, 0xBB]);
        assert_eq!(&out[34..37], &[0xCC, 0xDD, 0xEE]);
    }

    fn test_empty_code_and_data() {
        let out = write(0x1234, &[], &[]);
        assert_eq!(out.len(), 32);
    }

    fn test_header_is_32_bytes() {
        // "Magic"(4) + "Version"(4) + "Entry"(8) + "CodeSize"(8) + "DataSize"(8) = 32
        assert_eq!(4 + 4 + 8 + 8 + 8, 32);
    }

    pub fn run() {
        let tests: &[&dyn crate::test::Test] = &[
            &test_magic_bytes,
            &test_version_field,
            &test_entry_address_roundtrip,
            &test_code_and_data_size_fields,
            &test_total_length_matches_header_plus_sections,
            &test_code_and_data_bytes_placed_correctly,
            &test_empty_code_and_data,
            &test_header_is_32_bytes,
        ];
        println!("Running {} tests", tests.len());
        for test in tests {
            test.run();
        }
        println!("OK");
    }
}
