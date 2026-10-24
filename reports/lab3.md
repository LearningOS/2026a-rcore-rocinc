# Lab3 (ch5) 实验报告

## 实验环境

- WSL2，QEMU 8.2.2（qemu-system-riscv64）
- SBI：RustSBI 0.4.1（框架自带的 `bootloader/rustsbi-qemu.bin`）

## 实现的功能

迁移了 sys_get_time、mmap 和 munmap，改为通过当前进程的 TCB 访问地址空间。实现了 spawn，直接用 ELF 创建子进程并建立父子关系，不复制父进程地址空间。实现了 stride 调度和 set_priority：每次选 stride 最小的就绪进程，加上 BIG_STRIDE / priority，比较时处理溢出。选做完成了写时复制、六种调度算法和多核支持。

## 必做部分说明

- `BIG_STRIDE` 取 2⁴⁰，stride 用 `u64` 保存。测例中优先级为 5～10，pass 在 10¹¹ 量级，整除误差可以忽略。`set_priority(isize::MAX)` 时整除结果为 0，取 1 作为 pass 的下限。
- stride 的比较按问答题第 3 题的方法处理溢出，见 `sched/src/stride.rs` 中 `Stride` 的 `Ord` 实现。调度器暴力扫描就绪队列取最小值。
- `ch5_stride` 中 max(次数/优先级) ÷ min(次数/优先级) 在单核、2 核、4 核下分别为 1.035、1.047、1.040。

## 简答作业

### 1. 8 位 stride 的溢出

不会轮到 p1。p2 执行后 stride 变为 250 + 10 = 260，8 位无符号数溢出后是 4。调度器比较 p1 的 255 和 p2 的 4，认为 p2 更小，于是继续调度 p2。p2 的 stride 实际上已经超过 p1，比较结果因为溢出被反转了。

### 2. 为什么 STRIDE_MAX − STRIDE_MIN ≤ BigStride / 2

优先级 ≥ 2 时，每个进程的 pass = BigStride / priority ≤ BigStride / 2。

用归纳法说明。开始时所有 stride 都是 0，差值为 0。假设某一时刻所有就绪进程的 stride 都落在 [S_min, S_min + BigStride / 2] 中。调度器选中 stride 为 S_min 的进程，给它加上 pass，新值不超过 S_min + BigStride / 2；其他进程不变，都不小于 S_min。因此新的最小值不小于 S_min，新的最大值不超过 S_min + BigStride / 2，差值仍然不超过 BigStride / 2。

### 3. 考虑溢出的 partial_cmp

```rust
use core::cmp::Ordering;

struct Stride(u64);

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // 真实差值不超过 BigStride / 2 < 2^63，回绕后的差值按有符号数解释即可
        Some((self.0.wrapping_sub(other.0) as i64).cmp(&0))
    }
}

impl PartialEq for Stride {
    fn eq(&self, other: &Self) -> bool {
        false
    }
}
```

两个 stride 的真实差值的绝对值不超过 BigStride / 2。只要 BigStride / 2 小于 2⁶³，`a - b` 在模 2⁶⁴ 下的结果按 `i64` 解释时，符号就是真实差值的符号。用题目中的 8 位例子验证：125 − 255 = 126（i8），为正，所以 125 > 255；129 − 255 = −126（i8），为负，所以 129 < 255。

内核调度器使用的就是这个比较方法。

## 选做一：写时复制

### 设计

- `MapArea` 中的物理页帧从 `FrameTracker` 改为 `Arc<FrameTracker>`，同一个页帧可以被多个地址空间持有，最后一个持有者释放时归还页帧。
- `fork` 调用 `MemorySet::from_existed_user` 时，用户可见的页面不再复制：父子进程的页表项都指向原页帧，并去掉 W 位。Trap 上下文所在的页面不带 U 位，内核直接通过物理地址写它，所以仍然立即复制。
- 用户写只读页面触发 Store Page Fault。`trap_handler` 调用 `copy_on_write`：页面所在区域本来可写、但页表项不可写时，才是写时复制页。如果当前地址空间是该页帧唯一的持有者（`Arc::get_mut` 成功），直接恢复 W 位；否则分配新页帧、复制内容、重新映射。其他情况仍然杀死进程，只读的 mmap 区域在 fork 后依然不可写。
- 内核通过物理地址访问用户内存，不经过用户页表，所以写时复制不会被触发。`sys_waitpid` 写退出码、`sys_get_time` 写 `TimeVal`、`sys_read` 写缓冲区之前，先调用 `prepare_write` 解除对应页面的共享。后续章节迁移时，新增的内核写用户内存的系统调用也要先调用它。

### 测试

测例 `tests/ch5/ch5x_cow.rs`：

- mmap 20 MiB 并逐页写入，然后 fork 6 个子进程。每个子进程读遍全部 5120 页，改写其中一页，并保持存活到所有子进程创建完毕。父进程在子进程存活期间也改写一页。最后检查各自的修改互不可见。7 个进程如果各自持有一份副本需要 140 MiB，超过 QEMU 的 128 MiB 物理内存。
- 父进程与一个存活的子进程共享页面时，让内核向这些页面写入退出码和 `TimeVal`，子进程检查自己看到的值没有变化。
- 对只读的 mmap 页面，fork 后子进程写入会被杀死，退出码为 −2。

结果如下。对照组把 `from_existed_user` 临时改回逐页复制：

| 指标 | 写时复制 | 逐页复制 |
| --- | --- | --- |
| 映射 20 MiB 后 10 次 fork + exit + waitpid | 22 ms | 366 ms |
| 6 个子进程共享 20 MiB | 通过 | 页帧耗尽，内核 panic |

`ch5b_usertest` 中的 fork 系列测例在单核和 4 核下也都通过。

## 选做二：多种调度算法

### 设计

调度器放在独立的 `sched` crate 中，`no_std`，不依赖内核。每个进程在 TCB 中保存一个 `Entity`，记录优先级、stride、MLFQ 层级等调度状态。`Scheduler` trait 有四个操作：进程就绪时 `push`，取下一个进程时 `pop`，选中后 `pick`，时钟中断时 `tick`。内核的 `TaskManager` 只负责调用它们。

实现了六种算法：

| 名称 | 算法 | `priority` 的含义 |
| --- | --- | --- |
| `fifo` | 时间片轮转 | 不使用 |
| `stride` | stride 调度（默认） | 权重 |
| `lottery` | 抽签调度，xorshift 伪随机数 | 票数 |
| `mlfq` | 4 级多级反馈队列。第 k 级的配额为 2ᵏ 个时钟中断，用完降一级；主动让出 CPU 不消耗配额；每 100 个时钟中断全部提升到最高级 | 不使用 |
| `rt` | 固定优先级实时调度，同优先级轮转 | 优先级，越大越优先 |
| `edf` | 最早截止时间优先。就绪时如果原截止时间已过，设为当前时刻加 `priority` | 相对截止时间（时钟中断数） |

算法在编译时通过环境变量 `SCHED` 选择：

```sh
SCHED=mlfq make -C os run BASE=2
```

### 测试

- 库的单元测试：`cd sched && cargo test`，共 10 个用例，覆盖 stride 跨越 u64 溢出后仍按比例分配、抽签的比例、MLFQ 的降级和周期提升、实时调度只运行最高优先级、EDF 的截止时间选择等，全部通过。
- 内核测例 `tests/ch5/ch5x_sched.rs`：沿用 `ch5_stride` 的 6 个 CPU 密集型子进程（优先级 5～10，各运行 4 s），父进程设优先级 4，在这 4 s 内不断 yield，记录 yield 次数和两次运行之间的最大间隔，用来观察交互式进程的响应。

单核结果：

| 算法 | max(次数/优先级) ÷ min(次数/优先级) | 父进程 yield 次数 | 最大间隔 |
| --- | --- | --- | --- |
| fifo | 1.94 | 66 | 61 ms |
| stride | 1.03 | 36 | 122 ms |
| lottery | 1.81 | 32 | 457 ms |
| mlfq | 1.96 | 10358 | 122 ms |
| rt | 384 | 1 | 4062 ms |
| edf | 2.25 | 59752 | 101 ms |

- stride 的比例最接近 1。fifo 和 mlfq 不看优先级，各进程运行次数相近，比值约等于 10 / 5。
- lottery 平均来看按票数分配，但 4 s 内只有约 400 个时间片，随机误差明显。单元测试中跑 70000 个时间片时，比值在 1.1 以内。
- mlfq 中，父进程每次都主动让出 CPU，一直留在最高级，yield 次数远多于 fifo；CPU 密集型进程沉到低层。
- rt 中，优先级 10 的进程独占 CPU，其他进程几乎没有运行，直到它退出。这是严格优先级调度的饥饿问题。
- edf 中父进程的相对截止时间最短（4），总是最先被选中。其他进程截止时间越短运行越多，所以比例和优先级成反比。

除 rt 外，五种算法下 `ch5_usertest` 均通过。rt 下，`ch5_usertest` 自身优先级为 16，它在 `waitpid` 中不断 yield，又总是最高优先级，`ch5_stride` 一直得不到运行。这同样是饥饿问题，评测使用默认的 stride。

## 选做三：多核支持

### 设计

内核本身不可抢占：S 态中断只在用户态开启，进入内核后关闭。各个 hart 共享一个全局就绪队列，从中取进程运行，所以进程可以在 hart 之间迁移。

- **启动**：RustSBI 随机选一个 hart 进入内核（实测 4 核时启动 hart 为 3），它完成初始化后，通过 SBI HSM 扩展的 `hart_start` 启动其余 hart。`entry.asm` 为每个 hart 分配 64 KiB 启动栈，并把 hart id 放进 `tp`。QEMU 的 `-smp` 由 `os/Makefile` 中的 `SMP` 变量控制，默认 1。
- **hart id**：内核用 `tp` 保存 hart id。`__restore` 返回用户态前把 `tp` 写进 Trap 上下文中原本空着的 x4 位置，`__alltraps` 进入内核时再读回来。进程迁移到别的 hart 后，下次 trap 读到的是那个 hart 的 id。代价是用户态的 `tp` 不再保留，框架原本也不保存它，用户程序不使用它。
- **锁**：`UPSafeCell` 保持原有接口，内部改为自旋锁，原来调用 `exclusive_access` 的地方不用修改。锁记录持有者的 hart id，同一个 hart 重复获取时直接 panic，报出类型、hart 和调用位置，与原来 `RefCell` 的行为一致，不会静默死锁。panic 处理函数如果发现本 hart 正持有控制台锁，就绕过锁直接输出，否则仍加锁，避免与其他 hart 的输出交错。
- **Processor**：每个 hart 一个 `Processor`，通过 `tp` 索引。
- **切换**：单核时，进程先放回就绪队列再切换上下文，不会出问题。多核时，另一个 hart 可能在上下文保存完之前就取走这个进程，两个 hart 会同时使用同一个内核栈。现在 `schedule` 只把进程和它的新状态（Ready 或 Zombie）记在本 hart 的 `Processor` 中，等 `__switch` 回到 idle 控制流后，由 `run_tasks` 设置状态、放回就绪队列。退出的进程也是在离开内核栈之后才变成 Zombie，父进程这时才能回收它的内核栈。
- **TLB**：内核栈的映射由各 hart 在内核地址空间中增删，其他 hart 的 TLB 可能还缓存着旧映射。`run_tasks` 在切换到新进程前执行一次 `sfence.vma`。
- **waitpid**：子进程可能在别的 hart 上刚退出，那里还持有它的 `Arc`，所以去掉了框架中引用计数为 1 的断言，由最后一个持有者释放。

### 测试

- `ch5_usertest` 和 `ch5b_usertest` 在 `SMP=1`、`2`、`4` 下全部通过。`ch5x_cow` 在 4 核下也通过。
- 测例 `tests/ch5/ch5x_smp.rs` fork 8 个进程，各做同样的 3 × 10⁸ 次整数运算，记录总耗时：

| SMP | 耗时 | 加速比 |
| --- | --- | --- |
| 1 | 2520 ms | 1.00 |
| 2 | 1247 ms | 2.02 |
| 4 | 649 ms | 3.88 |

### 运行选做测例

```sh
cp tests/ch5/*.rs user/src/bin/
SCHED=stride make -C os run BASE=2 SMP=4
```

启动后在 shell 中输入 `ch5x_cow`、`ch5x_sched` 或 `ch5x_smp`。

## 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

    > 无

2. 此外，我也参考了 **以下资料** ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

    > rCore-Tutorial-Guide 第五章及练习说明；RISC-V SBI 规范中 HSM 扩展的说明；RISC-V 特权级规范中 `sfence.vma` 和 page fault 的说明；《Operating Systems: Three Easy Pieces》第 8 章多级反馈队列的规则。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。
