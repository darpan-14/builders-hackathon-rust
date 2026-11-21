use memchr::{memchr, memchr_iter};
use memmap2::Mmap;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::{
    fs::{self, File},
    io,
    path::Path,
};

pub type Str<'a> = &'a [u8];
pub const DAY: i64 = 86_400_000;
pub const WEEK: i64 = 7 * DAY;

pub struct Files {
    pub events: Vec<Mmap>,
    pub clicks: Vec<Mmap>,
    pub links: Vec<Mmap>,
    products: Vec<u8>,
    campaigns: Vec<u8>,
}
fn parts(dir: &Path) -> io::Result<Vec<Mmap>> {
    let paths = fs::read_dir(dir)?
        .map(|f| f.map(|d| d.path()))
        .collect::<io::Result<Vec<_>>>()?;
    paths
        .par_iter()
        .filter(|p| p.extension().is_some_and(|v| v == "csv"))
        .map(|p| {
            // The evaluator mounts its input read-only for the lifetime of the process.
            unsafe { Mmap::map(&File::open(p)?) }
        })
        .collect()
}
impl Files {
    pub fn open(dir: &Path) -> io::Result<Self> {
        Ok(Self {
            events: parts(&dir.join("events"))?,
            clicks: parts(&dir.join("clicks"))?,
            links: parts(&dir.join("links"))?,
            products: fs::read(dir.join("products.csv"))?,
            campaigns: fs::read(dir.join("campaigns.csv"))?,
        })
    }
}
fn body(bytes: &[u8]) -> &[u8] {
    memchr(b'\n', bytes).map_or(&[], |i| &bytes[i + 1..])
}
pub fn blocks(files: &[Mmap]) -> Vec<&[u8]> {
    let mut out = vec![];
    for map in files {
        let mut data = body(map);
        while !data.is_empty() {
            let target = 512 * 1024;
            let end = if data.len() <= target {
                data.len()
            } else {
                memchr(b'\n', &data[target..]).map_or(data.len(), |n| target + n + 1)
            };
            out.push(&data[..end]);
            data = &data[end..];
        }
    }
    out
}
pub fn lines<'a>(bytes: &'a [u8], mut visit: impl FnMut(&'a [u8])) {
    let mut start = 0;
    for end in memchr_iter(b'\n', bytes) {
        visit(&bytes[start..end]);
        start = end + 1;
    }
    if start < bytes.len() {
        visit(&bytes[start..]);
    }
}
pub fn fields<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    let mut out = [&[][..]; N];
    let mut rest = line;
    for (i, slot) in out.iter_mut().enumerate() {
        let field = if i + 1 == N {
            if memchr(b',', rest).is_some() {
                return None;
            }
            rest
        } else {
            let end = memchr(b',', rest)?;
            let field = &rest[..end];
            rest = &rest[end + 1..];
            field
        };
        let value = if field.first() == Some(&b'"') {
            field[1..].strip_suffix(b"\"")?
        } else {
            field
        };
        if memchr(b'"', value).is_some() {
            return None;
        }
        *slot = value;
    }
    Some(out)
}
#[inline]
fn pair(x: &[u8], i: usize) -> Option<i64> {
    let a = x[i].wrapping_sub(b'0');
    let b = x[i + 1].wrapping_sub(b'0');
    (a < 10 && b < 10).then_some(i64::from(a) * 10 + i64::from(b))
}
pub fn number(x: &[u8]) -> Option<i64> {
    if x.is_empty() {
        return None;
    }
    let mut n = 0i64;
    for &c in x {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(d.into())?;
    }
    Some(n)
}
fn civil_day(y: i64, m: i64, d: i64) -> Option<i64> {
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28 + i64::from(leap),
        _ => return None,
    };
    if d == 0 || d > max {
        return None;
    }
    let year = y - i64::from(m <= 2);
    let era = year.div_euclid(400);
    let year = year - era * 400;
    Some(
        era * 146097 + year * 365 + year / 4 - year / 100
            + (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5
            + d
            - 1
            - 719468,
    )
}
fn generic_date(x: &[u8]) -> Option<i64> {
    if x[4] != b'-' || x[7] != b'-' {
        return None;
    }
    civil_day(pair(x, 0)? * 100 + pair(x, 2)?, pair(x, 5)?, pair(x, 8)?)
}
// Most ISO input repeats a few dates. Cache exact ten-byte prefixes, including punctuation.
pub struct Clock {
    days: [Option<([u8; 10], i64)>; 64],
}
impl Clock {
    pub fn new() -> Self {
        Self { days: [None; 64] }
    }
    #[inline]
    pub fn parse(&mut self, x: &[u8]) -> Option<i64> {
        if !matches!(x.len(), 20 | 25) || x[4] != b'-' {
            return number(x);
        }
        let date: [u8; 10] = x[..10].try_into().ok()?;
        let slot = (usize::from(x[8]) * 10 + usize::from(x[9])) & 63;
        let day = match self.days[slot] {
            Some((key, day)) if key == date => day,
            _ => {
                let day = generic_date(x)?;
                self.days[slot] = Some((date, day));
                day
            }
        };
        if x[10] != b'T' || x[13] != b':' || x[16] != b':' {
            return None;
        }
        let h = pair(x, 11)?;
        let m = pair(x, 14)?;
        let s = pair(x, 17)?;
        if h >= 24 || m >= 60 || s >= 60 {
            return None;
        }
        let mut seconds = h * 3600 + m * 60 + s;
        if x.len() == 20 {
            if x[19] != b'Z' {
                return None;
            }
        } else {
            if x[22] != b':' {
                return None;
            }
            let h = pair(x, 20)?;
            let m = pair(x, 23)?;
            if h >= 24 || m >= 60 {
                return None;
            }
            let offset = h * 3600 + m * 60;
            match x[19] {
                b'+' => seconds -= offset,
                b'-' => seconds += offset,
                _ => return None,
            }
        }
        Some(day * DAY + seconds * 1000)
    }
}
pub struct Catalog<'a> {
    pub products: FxHashMap<Str<'a>, u32>,
    pub categories: Vec<Str<'a>>,
    pub campaigns: FxHashMap<Str<'a>, u32>,
    pub channels: Vec<(Str<'a>, Str<'a>)>,
}
impl<'a> Catalog<'a> {
    pub fn new(files: &'a Files) -> Self {
        let mut cat = Self {
            products: FxHashMap::default(),
            categories: vec![b"unknown"],
            campaigns: FxHashMap::default(),
            channels: vec![(b"organic", b"organic")],
        };
        let mut ids = FxHashMap::default();
        lines(body(&files.products), |line| {
            if let Some([p, c]) = fields(line) {
                let id = *ids.entry(c).or_insert_with(|| {
                    cat.categories.push(c);
                    (cat.categories.len() - 1) as u32
                });
                cat.products.insert(p, id);
            }
        });
        lines(body(&files.campaigns), |line| {
            if let Some([id, ch]) = fields(line) {
                cat.campaigns.insert(id, cat.channels.len() as u32);
                cat.channels.push((id, ch));
            }
        });
        cat
    }
}
struct Forest<'a> {
    ids: FxHashMap<Str<'a>, usize>,
    parent: Vec<usize>,
    size: Vec<usize>,
    names: Vec<Str<'a>>,
}
impl<'a> Forest<'a> {
    fn intern(&mut self, name: Str<'a>) -> usize {
        *self.ids.entry(name).or_insert_with(|| {
            let id = self.parent.len();
            self.parent.push(id);
            self.size.push(1);
            self.names.push(name);
            id
        })
    }
    fn root(&mut self, mut p: usize) -> usize {
        while self.parent[p] != p {
            self.parent[p] = self.parent[self.parent[p]];
            p = self.parent[p];
        }
        p
    }
    fn join(&mut self, a: Str<'a>, b: Str<'a>) {
        let a = self.intern(a);
        let b = self.intern(b);
        let mut a = self.root(a);
        let mut b = self.root(b);
        if a == b {
            return;
        }
        if self.size[a] < self.size[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        self.size[a] += self.size[b];
        self.names[a] = self.names[a].min(self.names[b]);
    }
}
pub struct Aliases<'a> {
    pub names: Vec<Str<'a>>,
    pub ids: FxHashMap<Str<'a>, usize>,
}
pub fn aliases(files: &[Mmap]) -> Aliases<'_> {
    let mut forest = Forest {
        ids: FxHashMap::default(),
        parent: vec![],
        size: vec![],
        names: vec![],
    };
    for block in blocks(files) {
        lines(block, |line| {
            if let Some([a, b]) = fields(line)
                && !a.is_empty()
                && !b.is_empty()
            {
                forest.join(a, b);
            }
        });
    }
    let mut ids = std::mem::take(&mut forest.ids);
    for id in ids.values_mut() {
        *id = forest.root(*id);
    }
    Aliases {
        names: forest.names,
        ids,
    }
}
