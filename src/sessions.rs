use crate::{
    input::{Catalog, DAY, WEEK},
    scan::{Click, People},
};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

pub struct UserDay<'a> {
    pub name: crate::name::Name<'a>,
    pub day: i32,
    pub sessions: i64,
    pub events: i64,
    pub revenue: i64,
    pub category: Option<u32>,
}
#[derive(Default, Clone, Copy)]
pub struct Credit {
    pub sessions: i64,
    pub purchasing: i64,
    pub revenue: i64,
    pub assists: i64,
}
#[derive(Default)]
pub struct Daily<'a> {
    pub users: Vec<UserDay<'a>>,
    pub credits: FxHashMap<(u32, i32), Credit>,
}

struct Window {
    active: Vec<u32>,
    count: Vec<usize>,
    position: Vec<usize>,
    left: usize,
    right: usize,
}
impl Window {
    fn new(n: usize) -> Self {
        Self {
            active: vec![],
            count: vec![0; n],
            position: vec![0; n],
            left: 0,
            right: 0,
        }
    }
    fn clear(&mut self) {
        for c in self.active.drain(..) {
            self.count[c as usize] = 0;
        }
        self.left = 0;
        self.right = 0;
    }
    fn advance(&mut self, start: i64, clicks: &[Click]) -> u32 {
        while self.right < clicks.len() && clicks[self.right].ts <= start {
            let c = clicks[self.right].campaign as usize;
            self.right += 1;
            if self.count[c] == 0 {
                self.position[c] = self.active.len();
                self.active.push(c as u32);
            }
            self.count[c] += 1;
        }
        while self.left < self.right && clicks[self.left].ts < start - WEEK {
            let c = clicks[self.left].campaign as usize;
            self.left += 1;
            self.count[c] -= 1;
            if self.count[c] == 0 {
                let i = self.position[c];
                self.active.swap_remove(i);
                if let Some(&moved) = self.active.get(i) {
                    self.position[moved as usize] = i;
                }
            }
        }
        if self.left == self.right {
            0
        } else {
            clicks[self.right - 1].campaign
        }
    }
}
pub fn summarize<'a>(people: People<'a>, catalog: &Catalog) -> Vec<Daily<'a>> {
    people
        .into_par_iter()
        .map(|mut shard| {
            let mut out = Daily::default();
            let mut window = Window::new(catalog.channels.len());
            let mut amounts = vec![0i64; catalog.categories.len()];
            let mut purchased = vec![];
            let (mut event_start, mut click_start) = (0, 0);
            for (p, name) in shard.names.into_iter().enumerate() {
                let event_end = shard.events.ends[p];
                let click_end = shard.clicks.ends[p];
                let events = &mut shard.events.rows[event_start..event_end];
                let clicks = &mut shard.clicks.rows[click_start..click_end];
                (event_start, click_start) = (event_end, click_end);
                if events.is_empty() {
                    continue;
                }
                events.sort_unstable_by_key(|e| e.ts);
                // At a tied timestamp, the lexically smallest ID finishes last.
                clicks.sort_unstable_by(|a, b| a.ts.cmp(&b.ts).then_with(|| b.id.cmp(a.id)));
                // Copies are identical and therefore adjacent after sorting.
                let mut n = 0;
                for i in 0..clicks.len() {
                    if n == 0 || clicks[i].id != clicks[n - 1].id {
                        clicks[n] = clicks[i];
                        n += 1;
                    }
                }
                let clicks = &clicks[..n];
                window.clear();
                let mut i = 0;
                let mut day_row: Option<UserDay> = None;
                while i < events.len() {
                    let begin = i;
                    let start = events[i].ts;
                    let day = start.div_euclid(DAY) as i32;
                    if day_row.as_ref().is_some_and(|row| row.day != day) {
                        out.users.push(day_row.take().unwrap());
                        for c in purchased.drain(..) {
                            amounts[c] = 0;
                        }
                    }
                    let row = day_row.get_or_insert(UserDay {
                        name,
                        day,
                        sessions: 0,
                        events: 0,
                        revenue: 0,
                        category: None,
                    });
                    let mut revenue = 0;
                    loop {
                        let e = &events[i];
                        i += 1;
                        if e.price != 0 {
                            let c = e.category as usize;
                            if amounts[c] == 0 {
                                purchased.push(c);
                            }
                            amounts[c] += i64::from(e.price);
                            revenue += i64::from(e.price);
                            if row.category.is_none_or(|best| {
                                let b = best as usize;
                                amounts[c] > amounts[b]
                                    || (amounts[c] == amounts[b]
                                        && catalog.categories[c] < catalog.categories[b])
                            }) {
                                row.category = Some(e.category);
                            }
                        }
                        if i == events.len() || events[i].ts - e.ts > 1_800_000 {
                            break;
                        }
                    }
                    row.sessions += 1;
                    row.events += (i - begin) as i64;
                    row.revenue += revenue;
                    let winner = window.advance(start, clicks);
                    let credit = out.credits.entry((winner, day)).or_default();
                    credit.sessions += 1;
                    credit.purchasing += i64::from(revenue > 0);
                    credit.revenue += revenue;
                    for &campaign in &window.active {
                        out.credits.entry((campaign, day)).or_default().assists += 1;
                    }
                }
                if let Some(row) = day_row {
                    out.users.push(row);
                }
                for c in purchased.drain(..) {
                    amounts[c] = 0;
                }
            }
            out
        })
        .collect()
}
