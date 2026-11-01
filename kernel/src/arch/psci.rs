const PSCI_0_2_FN64_CPU_ON: u32 = 0xC4000003;

#[inline]
pub unsafe fn psci_cpu_on(target_cpu: u64, entry_point: u64, context_id: u64) -> i64 {
    let result: i64;
    core::arch::asm!(
        "hvc #0",
        inout("x0") PSCI_0_2_FN64_CPU_ON as u64 => result,
        in("x1") target_cpu,
        in("x2") entry_point,
        in("x3") context_id,
        options(nomem, nostack)
    );
    result
}

pub fn get_core_id() -> u64 {
    unsafe {
        let id: u64;
        core::arch::asm!("mrs x0, mpidr_el1", "and x0, x0, #0xFF", out("x0") id, options(nomem, nostack));
        id
    }
}
