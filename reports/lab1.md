# Lab1 (ch3) 实验报告

## 实验环境

- WSL2，QEMU 8.2.2（qemu-system-riscv64）
- SBI：RustSBI 0.4.1（框架自带的 `bootloader/rustsbi-qemu.bin`）

## 实现的功能

实现了 410 号系统调用 `sys_trace`。请求 0 和 1 把 `id` 当作当前任务的地址，直接读出或写入一个字节；请求 2 返回当前任务调用某个系统调用的次数；其他请求返回 -1。

计数放在 `TaskControlBlock` 里，每个任务一个长度为 `MAX_SYSCALL_NUM` 的数组。`syscall()` 在分发之前先给当前任务对应的计数加一，所以查询时本次 `sys_trace` 也已计入。

## 简答作业

### 1. bad 测例的出错行为

用 `make -C os run TEST=2 BASE=1` 运行三个 `ch2b_bad_*` 测例，内核输出如下：

```text
[kernel] PageFault in application, bad addr = 0x0, bad instruction = 0x804003a4, kernel killed it.
[kernel] IllegalInstruction in application, kernel killed it.
[kernel] IllegalInstruction in application, kernel killed it.
```

- `ch2b_bad_address` 向地址 0x0 写数据，触发 Store 访存异常。`stval` 是出错地址 0x0，`sepc` 是出错指令的地址。
- `ch2b_bad_instructions` 在 U 态执行 `sret`，这是 S 态特权指令，触发非法指令异常。
- `ch2b_bad_register` 在 U 态用 `csrr` 读 `sstatus`，U 态无权访问 S 态 CSR，同样触发非法指令异常。

三种情况都会陷入 S 态，由 `trap_handler` 杀掉当前应用，然后切换到下一个应用继续运行，内核本身不受影响。

### 2. `__alltraps` 和 `__restore`

`__alltraps` 在 trap 进入 S 态时，把用户栈切换成内核栈，在内核栈上保存 `TrapContext`，再调用 `trap_handler`。`__restore` 做相反的事：从 `TrapContext` 恢复 CSR 和通用寄存器，切回用户栈，然后用 `sret` 返回 U 态。

**(1)** 刚进入 `__restore` 时，`sp` 指向当前任务内核栈上的 `TrapContext`。它有两种使用场景：

- 应用第一次运行。`TaskContext::goto_restore` 把 `ra` 设为 `__restore`，把 `sp` 设为 `init_app_cx` 在内核栈上压入的初始 `TrapContext`。`__switch` 执行 `ret` 后跳到 `__restore`，从 `sepc` 中的应用入口开始运行。
- trap 处理结束后返回用户态。`trap_handler` 返回后，控制流顺序落到 `__restore`，此时 `sp` 仍指向 `__alltraps` 保存的那个 `TrapContext`。

**(2)** 这几行恢复了三个 CSR：

- `sstatus`：其中 `SPP` 字段为 User，决定 `sret` 返回到 U 态；`SPIE` 决定返回后是否开启 S 态中断。
- `sepc`：`sret` 之后跳转的地址。第一次运行时是应用入口，系统调用返回时是 `ecall` 的下一条指令。
- `sscratch`：写入保存的用户栈指针，供 L60 换栈使用。

**(3)** `x2` 就是 `sp`，它此刻还在充当访问 `TrapContext` 的基址，不能被覆盖；用户栈指针已经放进 `sscratch`，最后由 `csrrw` 换回。`x4` 是 `tp`，应用程序不使用它，所以 `__alltraps` 没有保存，这里也不必恢复。

**(4)** `sp` 变为用户栈指针，供返回后的应用使用。`sscratch` 变为释放 `TrapContext` 之后的内核栈指针，供下次 trap 时换栈。

**(5)** 状态切换发生在 `sret`。`sret` 把特权级设为 `sstatus.SPP` 中的值，此时为 User；把 `SIE` 恢复为 `SPIE`；再把 `pc` 设为 `sepc`。因此执行后处理器回到 U 态，从 `sepc` 处继续执行。

**(6)** 与 L60 相反。`sp` 变为当前任务的内核栈指针，`sscratch` 保存陷入前的用户栈指针，之后才能在内核栈上分配 `TrapContext`。

**(7)** 系统调用是由用户程序执行的 `ecall` 进入 S 态的。硬件产生 Environment call from U-mode 异常，转到 `stvec` 指向的 `__alltraps`。此外，访存异常、非法指令和本章的时钟中断也会让 U 态陷入 S 态，这些情况不需要特定的指令。

## 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

    > 无

2. 此外，我也参考了 **以下资料** ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

    > rCore-Tutorial-Guide 第三章及练习说明；RISC-V 特权级规范中 `sstatus`、`sepc`、`sscratch` 和 `sret` 的说明。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。
