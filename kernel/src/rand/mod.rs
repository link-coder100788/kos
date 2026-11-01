//! Random numbers.
//!
//! ARMv8.5 added `RNDR`, a system register that returns 64 random bits
//! each time it's read, seeded from a hardware entropy source (under QEMU,
//! the host's). It's slow and can briefly run dry, so it only seeds a
//! ChaCha20 generator, which makes everything handed out here (the way
//! Linux's /dev/urandom works). The generator takes a fresh seed every
//! `RESEED_BYTES`, so its state at any moment says little about the rest.

use core::arch::asm;
use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use crate::std::lock::irq_mutex::IrqMutex;

/// Output between seeds from `RNDR`.
const RESEED_BYTES: usize = 1 << 20;
/// Output per turn of the lock, so a big request doesn't keep a core's
/// interrupts masked for long.
const CHUNK: usize = 4096;

struct Generator {
    chacha: ChaCha20Rng,
    until_reseed: usize,
}

impl Generator {
    fn seeded() -> Self {
        let mut seed = [0u8; 32];
        for chunk in seed.chunks_mut(8) {
            chunk.copy_from_slice(&hardware_u64().to_le_bytes());
        }
        Self { chacha: ChaCha20Rng::from_seed(seed), until_reseed: RESEED_BYTES }
    }
}

/// Seeded on first use.
static GENERATOR: IrqMutex<Option<Generator>> = IrqMutex::new(None);

/// Whether this CPU has `RNDR`: ID_AA64ISAR0_EL1 bits [63:60] are nonzero.
pub fn available() -> bool {
    let isar0: u64;
    unsafe { asm!("mrs {0}, id_aa64isar0_el1", out(reg) isar0); }
    isar0 >> 60 != 0
}

/// 64 bits straight from `RNDR`. On a CPU without it the read is an
/// undefined instruction and traps, so check `available()` first.
pub fn hardware_u64() -> u64 {
    // RNDR can fail now and then if the entropy source is briefly out; it
    // then returns 0 and sets the Z flag. Retrying is the documented fix.
    for _ in 0..100 {
        let value: u64;
        let ok: u64;
        // `s3_3_c2_c4_0` is RNDR's raw encoding, so the assembler doesn't
        // need to be told the CPU supports it.
        unsafe { asm!("mrs {0}, s3_3_c2_c4_0", "cset {1}, ne", out(reg) value, out(reg) ok); }
        if ok == 1 {
            return value;
        }
    }
    panic!("RNDR kept failing; is the CPU missing FEAT_RNG? (QEMU needs -cpu max)");
}

/// Fills `buf` with random bytes. Needs `RNDR` (see `available()`) to seed.
pub fn fill(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(CHUNK) {
        let mut generator = GENERATOR.lock();
        let generator = generator.get_or_insert_with(Generator::seeded);
        if generator.until_reseed < chunk.len() {
            *generator = Generator::seeded();
        }
        generator.chacha.fill_bytes(chunk);
        generator.until_reseed -= chunk.len();
    }
}

/// 64 random bits.
pub fn u64() -> u64 {
    let mut bytes = [0u8; 8];
    fill(&mut bytes);
    u64::from_le_bytes(bytes)
}

/// The generator wrapped in the interface crypto libraries ask for.
pub struct Rng;

impl rand_core::RngCore for Rng {
    fn next_u32(&mut self) -> u32 { u64() as u32 }
    fn next_u64(&mut self) -> u64 { u64() }
    fn fill_bytes(&mut self, dest: &mut [u8]) { fill(dest) }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        fill(dest);
        Ok(())
    }
}

/// Marks `Rng` as good enough for keys, not just for games.
impl rand_core::CryptoRng for Rng {}

#[cfg(feature = "test")]
pub mod tests {
    use alloc::vec;
    use crate::arch::timer;

    fn test_rand_outputs_differ() {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        super::fill(&mut a);
        super::fill(&mut b);
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 32]);
        assert_ne!(super::u64(), super::u64());
    }

    fn test_rand_reseeds_mid_request() {
        let mut buf = vec![0u8; super::RESEED_BYTES + 3 * super::CHUNK + 5];
        super::fill(&mut buf);
        let until = super::GENERATOR.lock().as_ref().unwrap().until_reseed;
        assert!(until < super::RESEED_BYTES, "a fresh seed partway through, then some used");
        // Same bytes twice in a row, 8 at a time, would mean a stuck generator.
        assert!(buf.chunks(8).zip(buf.chunks(8).skip(1)).all(|(a, b)| a != b));
    }

    /// Not a check, a measurement: how much the generator saves.
    fn test_rand_is_faster_than_rndr() {
        let mut buf = vec![0u8; 256 * 1024];
        let started = timer::now();
        super::fill(&mut buf);
        let chacha = timer::now() - started;
        let started = timer::now();
        for chunk in buf.chunks_mut(8) {
            chunk.copy_from_slice(&super::hardware_u64().to_le_bytes());
        }
        let rndr = timer::now() - started;
        let us = |ticks: u64| ticks * 1_000_000 / timer::frequency();
        log::info!("256 KiB random: ChaCha20 {} us, RNDR {} us", us(chacha), us(rndr));
    }

    pub fn run() {
        crate::test::run_all(&[
            &test_rand_outputs_differ,
            &test_rand_reseeds_mid_request,
            &test_rand_is_faster_than_rndr,
        ]);
    }
}
