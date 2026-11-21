use crate::input::{self, Catalog, Clock, DAY, Str, WEEK};
use crate::name::Name;
use memchr::memchr;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHasher};
use std::{array, hash::Hasher};

const SHARDS: usize = 64;
type Buckets<T> = [Vec<T>; SHARDS];
fn hash(key: &[u8]) -> usize {
    let mut h = FxHasher::default();
    h.write(key);
    h.finish() as usize
}
fn bucket(hash: usize) -> usize {
    hash & (SHARDS - 1)
}
fn lanes(len: usize) -> usize {
    len.div_ceil(2 * rayon::current_num_threads()).max(1)
}
fn transpose<T>(sources: impl IntoIterator<Item = Buckets<T>>) -> [Vec<Vec<T>>; SHARDS] {
    let mut dest: [Vec<Vec<T>>; SHARDS] = array::from_fn(|_| Vec::new());
    for source in sources {
        for (v, part) in dest.iter_mut().zip(source) {
            if !part.is_empty() {
                v.push(part);
            }
        }
    }
    dest
}
#[derive(Clone, Copy, Default)]
pub struct Event {
    pub ts: i64,
    pub price: u32,
    pub category: u32,
}
#[derive(Clone, Copy, Default)]
pub struct Click<'a> {
    pub ts: i64,
    pub campaign: u32,
    pub id: Str<'a>,
}
pub struct Grouped<T> {
    pub rows: Vec<T>,
    pub ends: Vec<usize>,
}
pub struct Partition<'a> {
    pub names: Vec<Name<'a>>,
    pub events: Grouped<Event>,
    pub clicks: Grouped<Click<'a>>,
}
pub type People<'a> = Vec<Partition<'a>>;

// Counting-sort numeric person IDs so all of one person's rows occupy a slice.
fn group<T: Copy + Default>(sources: Vec<Vec<(usize, T)>>, people: usize) -> Grouped<T> {
    let mut ends = vec![0; people];
    for rows in &sources {
        for (id, _) in rows {
            ends[*id] += 1;
        }
    }
    let mut count = 0;
    for end in &mut ends {
        let n = *end;
        *end = count;
        count += n;
    }
    let mut rows = vec![T::default(); count];
    for (id, row) in sources.into_iter().flatten() {
        rows[ends[id]] = row;
        ends[id] += 1;
    }
    Grouped { rows, ends }
}
struct SeenTwice {
    words: Vec<u64>,
}
impl SeenTwice {
    fn location(h: usize, mask: usize) -> (usize, u64) {
        (
            (h >> 6) & mask,
            1u64 << (h >> 58) | 1u64 << ((h >> 52) & 63),
        )
    }
    fn build(groups: Vec<Vec<usize>>) -> Self {
        let words = (groups.iter().map(Vec::len).sum::<usize>() / 6 + 1).next_power_of_two();
        let mut seen = vec![0; words];
        let mut repeated = vec![0; words];
        for h in groups.into_iter().flatten() {
            let (w, bits) = Self::location(h, words - 1);
            if seen[w] & bits == bits {
                repeated[w] |= bits;
            }
            seen[w] |= bits;
        }
        Self { words: repeated }
    }
    fn possible(&self, h: usize) -> bool {
        let (w, bits) = Self::location(h, self.words.len() - 1);
        self.words[w] & bits == bits
    }
}
// A malformed row can add a false positive here. Only fully parsed rows compete
// in the exact earliest-ingest map. No occurrence of a repeated valid ID can skip it.
fn repeated(blocks: &[&[u8]]) -> Vec<SeenTwice> {
    let hashes: Vec<Buckets<usize>> = blocks
        .par_chunks(lanes(blocks.len()))
        .map(|parts| {
            let mut shards: Buckets<usize> = array::from_fn(|_| vec![]);
            for part in parts {
                input::lines(part, |line| {
                    if let Some(end) = memchr(b',', line) {
                        let raw = &line[..end];
                        let id = raw
                            .strip_prefix(b"\"")
                            .and_then(|s| s.strip_suffix(b"\""))
                            .unwrap_or(raw);
                        let h = hash(id);
                        shards[bucket(h)].push(h);
                    }
                });
            }
            shards
        })
        .collect();
    transpose(hashes)
        .into_par_iter()
        .map(SeenTwice::build)
        .collect()
}
struct Candidate<'a> {
    id: Str<'a>,
    ingest: i64,
    event: Option<(Name<'a>, Event)>,
}
type NamedEvents<'a> = Buckets<(Name<'a>, Event)>;
type NamedClicks<'a> = Buckets<(Name<'a>, Click<'a>)>;
#[inline]
fn validate<'a>(
    f: &[Str<'a>; 7],
    ingest: i64,
    clock: &mut Clock,
    window: (i64, i64),
    catalog: &Catalog,
) -> Option<(Name<'a>, Event)> {
    if f[1].is_empty() || f[5].is_empty() {
        return None;
    }
    let (price, category) = match f[4] {
        b"view" | b"cart" => (0, 0),
        b"purchase" => {
            let p = input::number(f[6])?;
            if !(1..=10_000_000).contains(&p) {
                return None;
            }
            (p as u32, catalog.products.get(f[5]).copied().unwrap_or(0))
        }
        _ => return None,
    };
    let ts = clock.parse(f[2])?;
    if ts < window.0 || ts >= window.1 || ingest - ts > DAY {
        return None;
    }
    Some((
        Name::new(f[1]),
        Event {
            ts,
            price,
            category,
        },
    ))
}
fn events<'a>(blocks: &[Str<'a>], window: (i64, i64), catalog: &Catalog) -> Vec<NamedEvents<'a>> {
    let filters = repeated(blocks);
    let lanes: Vec<_> = blocks
        .par_chunks(lanes(blocks.len()))
        .map(|parts| {
            let mut accepted: NamedEvents = array::from_fn(|_| vec![]);
            let mut candidates: Buckets<Candidate> = array::from_fn(|_| vec![]);
            let mut clock = Clock::new();
            for part in parts {
                crate::csv::rows::<7>(part, |f| {
                    if f[0].is_empty() {
                        return;
                    }
                    let Some(ingest) = clock.parse(f[3]) else {
                        return;
                    };
                    let event = validate(&f, ingest, &mut clock, window, catalog);
                    let h = hash(f[0]);
                    if filters[bucket(h)].possible(h) {
                        candidates[bucket(h)].push(Candidate {
                            id: f[0],
                            ingest,
                            event,
                        });
                    } else if let Some((user, ev)) = event {
                        accepted[bucket(user.hash())].push((user, ev));
                    }
                });
            }
            (accepted, candidates)
        })
        .collect();
    let (mut accepted, candidates): (Vec<_>, Vec<_>) = lanes.into_iter().unzip();
    let extras: Vec<_> = transpose(candidates)
        .into_par_iter()
        .map(|groups| {
            let mut earliest = FxHashMap::default();
            for candidate in groups.into_iter().flatten() {
                let current = earliest.entry(candidate.id).or_insert((i64::MAX, None));
                if candidate.ingest < current.0 {
                    *current = (candidate.ingest, candidate.event);
                }
            }
            let mut out: NamedEvents = array::from_fn(|_| vec![]);
            for (user, ev) in earliest.into_values().filter_map(|(_, v)| v) {
                out[bucket(user.hash())].push((user, ev));
            }
            out
        })
        .collect();
    accepted.extend(extras);
    accepted
}
fn clicks<'a>(blocks: &[Str<'a>], window: (i64, i64), catalog: &Catalog) -> Vec<NamedClicks<'a>> {
    blocks
        .par_chunks(lanes(blocks.len()))
        .map(|parts| {
            let mut out: NamedClicks = array::from_fn(|_| vec![]);
            let mut clock = Clock::new();
            for part in parts {
                crate::csv::rows::<4>(part, |[id, user, campaign, time]| {
                    if id.is_empty() || user.is_empty() {
                        return;
                    }
                    let Some(&campaign) = catalog.campaigns.get(campaign) else {
                        return;
                    };
                    let Some(ts) = clock.parse(time) else { return };
                    if ts >= window.0 - WEEK && ts < window.1 {
                        let user = Name::new(user);
                        out[bucket(user.hash())].push((user, Click { id, ts, campaign }));
                    }
                });
            }
            out
        })
        .collect()
}
pub fn prepare<'a>(files: &'a input::Files, catalog: &Catalog, window: (i64, i64)) -> People<'a> {
    let ev = input::blocks(&files.events);
    let cl = input::blocks(&files.clicks);
    let (aliases, (events, clicks)) = rayon::join(
        || input::aliases(&files.links),
        || {
            rayon::join(
                || events(&ev, window, catalog),
                || clicks(&cl, window, catalog),
            )
        },
    );
    crate::phase("scan, deduplicate and link");
    let mut maps: [FxHashMap<Name, usize>; SHARDS] = array::from_fn(|_| FxHashMap::default());
    for (name, id) in aliases.ids {
        let name = Name::new(name);
        maps[bucket(name.hash())].insert(name, id);
    }
    let base = aliases.names.len().div_ceil(SHARDS);
    let mut names: [Vec<Name>; SHARDS] = array::from_fn(|_| vec![Name::Other(&[]); base]);
    for (i, n) in aliases.names.into_iter().enumerate() {
        names[bucket(i)][i / SHARDS] = Name::new(n);
    }
    let parts: Vec<_> = maps
        .into_par_iter()
        .zip(transpose(events))
        .zip(transpose(clicks))
        .enumerate()
        .map(|(s, ((mut ids, ev), cl))| {
            let mut extra = Vec::new();
            let mut resolve = |name: Name<'a>| {
                *ids.entry(name).or_insert_with(|| {
                    let id = (base + extra.len()) * SHARDS + s;
                    extra.push(name);
                    id
                })
            };
            let mut e: Buckets<(usize, Event)> = array::from_fn(|_| vec![]);
            let mut c: Buckets<(usize, Click)> = array::from_fn(|_| vec![]);
            for (user, row) in ev.into_iter().flatten() {
                let id = resolve(user);
                e[bucket(id)].push((id / SHARDS, row));
            }
            for (user, row) in cl.into_iter().flatten() {
                let id = resolve(user);
                c[bucket(id)].push((id / SHARDS, row));
            }
            (extra, e, c)
        })
        .collect();
    let mut e = Vec::new();
    let mut c = Vec::new();
    for (s, (extra, events, clicks)) in parts.into_iter().enumerate() {
        names[s].extend(extra);
        e.push(events);
        c.push(clicks);
    }
    names
        .into_par_iter()
        .zip(transpose(e))
        .zip(transpose(c))
        .map(|((names, ev), cl)| Partition {
            events: group(ev, names.len()),
            clicks: group(cl, names.len()),
            names,
        })
        .collect()
}
