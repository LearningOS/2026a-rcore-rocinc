    .section .text.entry
    .globl _start
_start:
    la ra, rust_main
    j boot

    .globl _start_secondary
_start_secondary:
    la ra, rust_main_secondary

boot:
    # a0 = hartid, each hart has a 64 KiB boot stack
    mv tp, a0
    addi t0, a0, 1
    slli t0, t0, 16
    la sp, boot_stack_lower_bound
    add sp, sp, t0
    jr ra

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16 * 8
    .globl boot_stack_top
boot_stack_top:
