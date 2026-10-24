use sched::*;

/// Run `slices` time slices; task `i` yields before the timer if `interactive[i]`.
/// Returns how many slices each task got.
fn simulate(
    s: &mut dyn Scheduler<usize>,
    prio: &[usize],
    interactive: &[bool],
    slices: usize,
) -> Vec<usize> {
    let mut entities: Vec<Entity> = prio
        .iter()
        .map(|&p| {
            let mut e = Entity::new();
            e.priority = p;
            e
        })
        .collect();
    for (i, e) in entities.iter_mut().enumerate() {
        s.push(i, e);
    }
    let mut runs = vec![0; prio.len()];
    for _ in 0..slices {
        let i = s.pop().unwrap();
        s.pick(&mut entities[i]);
        runs[i] += 1;
        if !interactive[i] {
            s.tick(&mut entities[i]);
        }
        s.push(i, &mut entities[i]);
    }
    runs
}

fn max_min_ratio(runs: &[usize], prio: &[usize]) -> f64 {
    let r: Vec<f64> = runs
        .iter()
        .zip(prio)
        .map(|(&n, &p)| n as f64 / p as f64)
        .collect();
    r.iter().cloned().fold(f64::MIN, f64::max) / r.iter().cloned().fold(f64::MAX, f64::min)
}

#[test]
fn stride_compares_across_overflow() {
    let near_max = Stride(u64::MAX - 10);
    let wrapped = Stride(5);
    assert!(near_max < wrapped);
    assert!(Stride(3) < Stride(9));
    assert!(Stride(BIG_STRIDE / 2) > Stride(0));
}

#[test]
fn stride_is_proportional() {
    let prio = [5, 6, 7, 8, 9, 10];
    let runs = simulate(&mut StrideScheduler::new(), &prio, &[false; 6], 45_000);
    assert!(max_min_ratio(&runs, &prio) < 1.01, "{runs:?}");
}

#[test]
fn stride_survives_wrap_around() {
    let mut s = StrideScheduler::new();
    let prio = [2, 3];
    let mut entities = [Entity::new(), Entity::new()];
    for (i, e) in entities.iter_mut().enumerate() {
        e.priority = prio[i];
        // start right before the wrap
        for _ in 0..(u64::MAX / (BIG_STRIDE / prio[i] as u64)) {
            s.pick(e);
        }
        s.push(i, e);
    }
    let mut runs = [0; 2];
    for _ in 0..5000 {
        let i = s.pop().unwrap();
        s.pick(&mut entities[i]);
        runs[i] += 1;
        s.push(i, &mut entities[i]);
    }
    assert!(max_min_ratio(&runs, &prio) < 1.01, "{runs:?}");
}

#[test]
fn lottery_is_roughly_proportional() {
    let prio = [5, 10, 20];
    let runs = simulate(&mut Lottery::new(), &prio, &[false; 3], 70_000);
    assert!(max_min_ratio(&runs, &prio) < 1.1, "{runs:?}");
}

#[test]
fn fifo_round_robin() {
    let runs = simulate(&mut Fifo::new(), &[2, 30, 7], &[false; 3], 300);
    assert_eq!(runs, [100, 100, 100]);
}

#[test]
fn mlfq_favors_interactive_tasks() {
    // one interactive task against three CPU-bound ones
    let runs = simulate(
        &mut Mlfq::new(),
        &[16; 4],
        &[true, false, false, false],
        1000,
    );
    assert!(runs[0] > runs[1] + runs[2] + runs[3], "{runs:?}");
}

#[test]
fn mlfq_demotes_and_boosts() {
    let mut s = Mlfq::new();
    let mut e = Entity::new();
    s.push(0usize, &mut e);
    // a CPU-bound task sinks to the bottom: 1 + 2 + 4 ticks
    for _ in 0..7 {
        s.pop().unwrap();
        s.pick(&mut e);
        s.tick(&mut e);
        s.push(0, &mut e);
    }
    let mut fresh = Entity::new();
    s.push(1, &mut fresh);
    assert_eq!(s.pop(), Some(1));
    // the boost lifts task 0 back to the top level, ahead of task 1
    let mut other = Entity::new();
    for _ in 7..100 {
        s.tick(&mut other);
    }
    s.push(1, &mut Entity::new());
    assert_eq!(s.pop(), Some(0));
}

#[test]
fn fixed_priority_runs_highest_first() {
    let runs = simulate(&mut FixedPriority::new(), &[3, 9, 9, 1], &[false; 4], 100);
    assert_eq!(runs, [0, 50, 50, 0]);
}

#[test]
fn edf_runs_earliest_deadline() {
    let mut s = Edf::new();
    let mut a = Entity::new();
    let mut b = Entity::new();
    a.priority = 50;
    b.priority = 5;
    s.push(0usize, &mut a);
    s.push(1usize, &mut b);
    assert_eq!(s.pop(), Some(1));
    // b keeps its deadline 5 until it passes, then gets 5 more ticks
    for _ in 0..5 {
        s.tick(&mut b);
    }
    s.push(1, &mut b);
    assert_eq!(s.pop(), Some(1));
    for _ in 0..50 {
        s.tick(&mut b);
    }
    s.push(1, &mut b);
    assert_eq!(s.pop(), Some(0));
}

#[test]
fn by_name_knows_all_schedulers() {
    for name in ["fifo", "stride", "lottery", "mlfq", "rt", "edf"] {
        assert!(by_name::<usize>(name).is_some());
    }
    assert!(by_name::<usize>("cfs").is_none());
}
