# Lab5 (ch8) 实验报告

## 实验环境

- WSL2，QEMU 8.2.2（qemu-system-riscv64）
- SBI：RustSBI 0.4.1（框架自带的 `bootloader/rustsbi-qemu.bin`）

## 实现的功能

迁移了 sys_get_time，按页拷贝 TimeVal。实现了 sys_enable_deadlock_detect 和基于银行家算法安全性检查的死锁检测：mutex 和 semaphore 各维护一套 Available、Allocation、Need，加锁或 down 前登记请求并检查，不安全时撤销登记并返回 −0xDEAD。ch5、ch6 的选做移植到线程化的框架上，继续可用。

## 必做部分说明

- **数据结构**：`sync/banker.rs` 中的 `Banker`，行是线程（按 tid），列是资源（按 mutex 或 semaphore 的 id）。进程控制块中有 `mutex_banker`、`sem_banker` 两份，以及检测开关 `deadlock_detect`。
- **登记与检查**：`mutex_lock`、`semaphore_down` 先把 Need[tid][id] 加 1，开关打开时运行安全性检查，不安全就撤销这次登记，返回 −0xDEAD，不阻塞。开关关闭时照常登记，中途打开检测时矩阵仍然正确。
- **获得与释放**：获得资源后（包括阻塞后被唤醒），Need 减 1，Allocation 加 1，Available 减 1。`mutex_unlock`、`semaphore_up` 使 Allocation 减 1，Available 加 1。信号量可以由其他线程 up，例如屏障，此时 up 的线程 Allocation 为 0，截断在 0。
- **矩阵扩展**：新建 mutex 或 semaphore 时扩展列，并把这一列设为初值：mutex 为 1，semaphore 为初始计数。id 被复用时同样重置。tid 超出行数时扩展行，`thread_create` 复用 tid 时把这一行清零。
- **主线程**：同样计入矩阵。`ch8_deadlock_sem1`、`sem2` 中主线程先把屏障信号量 down 到 0，再由子线程 up 回去；不记录主线程的占用，`sem2` 会被误判为死锁。
- **条件变量**：wait 期间释放 mutex，返回前重新持有，矩阵中也是先释放、再获得，重新获得时不检查。
- **sys_get_time**：通过 `copy_to_user` 按页拷贝 `TimeVal`，处理跨页的情况。

## 简答作业

### 1. 主线程退出时的资源回收

需要回收的资源：

- 所有线程的 `TaskUserRes`：tid，以及用户栈和 Trap 上下文所在的页面。要先于地址空间回收，否则这些页面会被释放两次。
- 地址空间：程序段、mmap 区域和页表。
- 文件描述符表。子进程转交给 initproc。
- mutex、semaphore、condvar 列表，以及阻塞在其中的线程。
- 各线程的内核栈随 TCB 释放。pid 和进程控制块保留到父进程 waitpid。

其他线程的 TCB 可能在以下位置被引用：

- **进程的 `tasks` 列表**：清空。它是线程的所有者，不清空的话 TCB 要等到 waitpid 释放进程控制块才回收。
- **就绪队列**：必须去掉，否则这些线程以后会被调度，在已回收的地址空间上运行。框架用 `remove_task` 扫描删除。我的实现给线程打上 killed 标记，被取出时直接丢弃。
- **sys_sleep 的定时器队列**：必须删除（`remove_timer`）。否则到期后线程会被放回就绪队列，问题同上，定时器的引用也会让 TCB 一直不释放。
- **mutex/semaphore/condvar 的等待队列**：这些线程不会再被唤醒，不会出错，但会一直占用 TCB 和内核栈。等待队列属于进程控制块，随它一起释放；进程控制块在 waitpid 之前以僵尸状态存在，所以我在退出时直接清空这三个列表，提前释放。
- **`Processor.current`**：单核时只有正在退出的主线程自己，它已由 `take_current_task` 取出。它的内核栈仍在使用，不能立即释放：框架交给 `add_stopping_task` 暂存，我的实现由 `schedule` 暂存在本核 `Processor` 中，切换到空闲控制流后再释放。多核时其他线程可能正在别的 hart 上运行，见选做部分。

### 2. Mutex1 与 Mutex2 的区别

- **Mutex1**：unlock 先把 `locked` 置为 false，再唤醒一个等待者，被唤醒的线程回到 lock 的循环中重新抢锁。从唤醒到它真正运行之间锁是空闲的，其他线程可以先抢到，包括刚 unlock 又立刻 lock 的同一个线程。被唤醒的线程抢不到时重新排到队尾，可能一直拿不到锁而饥饿，每次失败还多一次阻塞和切换。
- **Mutex2**：unlock 时如果有等待者，不清除 `locked`，锁直接交给被唤醒的线程，它从 lock 返回时已经持有锁，不再检查。没有等待者时才置为 false。等待者按 FIFO 顺序获得锁，不会饥饿。问题是锁交给了还没运行的线程，即使 CPU 空闲、其他线程想要锁，也只能等它被调度，吞吐量下降，容易形成锁护航。它还要求每次唤醒都意味着获得锁：如果被唤醒的线程被杀死而没有 unlock，锁永远处于占用状态；如果出现虚假唤醒，线程会误以为自己持有锁。
- 两者都直接调用 `add_task`，没有把线程状态改回 Ready，用框架的 `wakeup_task` 更合适。框架的 `MutexBlocking` 采用 Mutex2 的方式。
- 多核下两者还有一个共同的问题：等待者把自己放进等待队列后、真正切换出去之前，另一个 hart 可能已经 unlock 并把它放回就绪队列，同一个线程会在两个 hart 上同时运行。我的 `wakeup_task` 发现目标仍在运行时只做标记，等它切换出去后再放回就绪队列。

## 选做：ch5、ch6 选做的延续

ch8 框架改动很大：TCB 拆成进程和线程两层，`UPSafeCell` 回到 `RefCell`，easy-fs 模板也有改动。各选做按原设计重新合入，并针对多线程做了调整。

### 多核

- `UPSafeCell` 仍是 ch5 的自旋锁，记录持有者 hart，同一 hart 重复获取时 panic 并给出调用位置。框架中的进程、线程、等待队列、定时器、PID 表都经过它访问。
- 每个 hart 一个 `Processor`。切换出去的线程连同它离开时的状态（Ready、Blocked、Exited）暂存在本核 `Processor` 中，切换到空闲控制流后才设置状态、放回就绪队列或释放，其他 hart 不会在它的内核栈仍在使用时运行或回收它。框架的 `add_stopping_task` 只能暂存一个线程，多核下不够用，已去掉。
- 每个线程有自己的 Trap 上下文。`__restore` 把 `tp`（hart id）写进去，`__alltraps` 从中恢复。
- **唤醒丢失**：线程把自己加入等待队列（mutex、semaphore、condvar、定时器）后，到真正切换出去之前，其他 hart 可能已经唤醒它。`wakeup_task` 发现目标仍是 Running 时只设置 `wakeup_pending`，由 `run_tasks` 在它切换出去后直接放回就绪队列。condvar 的 wait 改为先加入等待队列、再释放 mutex，否则 signal 可能落在两者之间而丢失。
- **初始化顺序**：新线程和新进程完全初始化后才放入就绪队列。`thread_create` 先设好 Trap 上下文，fork 先把子进程的返回值置 0，spawn 先建立父子关系。原来的顺序下，其他 hart 可能提前运行它们。
- **进程退出**：主线程给其他线程打上 killed 标记、删除定时器，然后等待正在其他 hart 上运行的线程离开 CPU。这些线程下次进入内核时（时钟中断间隔为 10 ms）发现标记，自行退出；就绪队列中的线程被取出时丢弃。之后才回收用户资源和地址空间。
- 加锁顺序固定为进程 → 线程。`waittid` 和线程退出时释放 `TaskUserRes` 都按这个顺序。

### 写时复制

- fork 仍只支持单线程进程。地址空间由进程的所有线程共享，Page Fault 在进程锁内处理。
- 一个线程写时复制换了页帧，同一进程的其他线程可能正在别的 hart 上运行，TLB 里还是旧页帧的映射，会读到旧数据。复制页帧后以及 munmap 后，通过 SBI RFENCE 扩展刷新各 hart 的 TLB。目标只包括已进入内核的 hart：RustSBI 对尚未完成启动的 hart 返回 invalid_param，启动阶段的 hart 在运行第一个任务前会自己执行 sfence.vma。
- 反过来，其他 hart 可能因为旧的 TLB 项触发 Page Fault，而页表已经处理好了。`handle_page_fault` 发现页表项已允许这次访问时直接返回。
- 内核写用户内存前都调用 `prepare_write`，包括 ch8 新增的 pipe 写回 fd。

### 按需加载

- `from_elf` 接收 `Inode`，程序段在首次访问时加载。各线程的用户栈和 Trap 上下文由 `TaskUserRes` 立即分配。exec、spawn 和 initproc 都走这条路径。
- 内核读用户内存前先加载对应页面，包括 ch8 新增的 exec 参数数组和参数字符串。

### 日志文件系统

- 日志层按原设计合入 ch8 的 easy-fs 模板：33 块日志区，魔数 `0x3b800002`，每个修改操作在 efs 锁内以 `commit` 结束，单个事务不超过 32 块，块缓存 64 块且不换出被修改的块。
- linkat、unlinkat、fstat 一起恢复，`ch6_file0`～`ch6_file3` 可以运行。
- 宿主机测试 `cd easy-fs && cargo test`，5 个用例全部通过。

### 调度算法

`sched/` 原样带入，单元测试通过。调度实体从进程移到线程，每个线程一个 `Entity`。`set_priority` 设置当前进程所有线程的优先级。默认仍为 stride。

### 测试

新增测例 `tests/ch8/ch8x_threads.rs`：

- fork 出的子进程创建 4 个忙循环的线程，主线程 sleep 后退出。父进程检查退出码，这些线程随进程停止。
- 4 个线程同时写一块和子进程写时复制共享的 1 MiB 区域，每个线程写每一页中自己的一个字，同一页面会被多个线程同时触发写时复制。父进程检查所有线程写入的值都在，子进程看到的仍是 fork 前的内容。

`ch5x_cow` 中，向只读页面写入的子进程现在按 ch8 的信号机制以 SIGSEGV 退出，退出码为 −11，测例相应修改。

| 测例 | 单核 | 4 核 |
| --- | --- | --- |
| `ch8_usertest` | 通过 | 通过 |
| `ch8x_threads`、`ch5x_cow`、`ch6x_lazy`、`ch5x_sched` | 通过 | 通过 |
| `ch5x_smp`：8 个进程各做 3 × 10⁸ 次运算 | 2603 ms | 745 ms |

`ch8x_threads` 在 4 核下连续运行 80 次均通过。

### 运行选做测例

```sh
cp tests/ch5/*.rs tests/ch6/*.rs tests/ch8/*.rs user/src/bin/
make -C os run BASE=2 SMP=4
```

启动后在 shell 中输入 `ch8x_threads`、`ch5x_cow`、`ch6x_lazy` 等。ch8 框架没有 sbrk，`ch6_usertest` 中的 sbrk 测例无法运行，文件测例可以单独运行。

## 荣誉准则

1. 在完成本次实验的过程（含此前学习的过程）中，我曾分别与 **以下各位** 就（与本次实验相关的）以下方面做过交流，还在代码中对应的位置以注释形式记录了具体的交流对象及内容：

    > 无

2. 此外，我也参考了 **以下资料** ，还在代码中对应的位置以注释形式记录了具体的参考来源及内容：

    > rCore-Tutorial-Guide 第八章及练习说明；RISC-V SBI 规范 HSM 扩展与 RFENCE 扩展；RISC-V 特权级规范中 page fault 与 sfence.vma 的说明。

3. 我独立完成了本次实验除以上方面之外的所有工作，包括代码与文档。 我清楚地知道，从以上方面获得的信息在一定程度上降低了实验难度，可能会影响起评分。

4. 我从未使用过他人的代码，不管是原封不动地复制，还是经过了某些等价转换。 我未曾也不会向他人（含此后各届同学）复制或公开我的实验代码，我有义务妥善保管好它们。 我提交至本实验的评测系统的代码，均无意于破坏或妨碍任何计算机系统的正常运转。 我清楚地知道，以上情况均为本课程纪律所禁止，若违反，对应的实验成绩将按“-100”分计。
