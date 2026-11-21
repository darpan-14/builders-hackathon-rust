use crate::input::{fields, lines};
use memchr::{memchr, memrchr};

#[derive(Default)]
struct Masks {
    separators: u64,
    quotes: u64,
    newlines: u64,
}
#[inline]
fn masks(bytes: &[u8; 64]) -> Masks {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::*;
        let mut m = Masks::default();
        // x86-64 guarantees SSE2. The Docker build can process 32 bytes with AVX2.
        #[cfg(target_feature = "avx2")]
        unsafe {
            for j in [0, 32] {
                let v = _mm256_loadu_si256(bytes.as_ptr().add(j).cast());
                let eq = |c| {
                    u64::from(
                        _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, _mm256_set1_epi8(c))) as u32,
                    ) << j
                };
                m.separators |= eq(b',' as i8);
                m.quotes |= eq(b'"' as i8);
                m.newlines |= eq(b'\n' as i8);
            }
        }
        #[cfg(not(target_feature = "avx2"))]
        unsafe {
            for j in [0, 16, 32, 48] {
                let v = _mm_loadu_si128(bytes.as_ptr().add(j).cast());
                let eq = |c| (_mm_movemask_epi8(_mm_cmpeq_epi8(v, _mm_set1_epi8(c))) as u64) << j;
                m.separators |= eq(b',' as i8);
                m.quotes |= eq(b'"' as i8);
                m.newlines |= eq(b'\n' as i8);
            }
        }
        m.separators |= m.newlines;
        m
    }
    #[cfg(target_arch = "aarch64")]
    {
        use std::arch::aarch64::*;
        let mut m = Masks::default();
        // Each eight-byte half sums independent powers of two into one byte.
        unsafe {
            let powers =
                vld1q_u8([1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128].as_ptr());
            for j in [0, 16, 32, 48] {
                let bytes = vld1q_u8(bytes.as_ptr().add(j));
                let eq = |c| {
                    let bits = vandq_u8(vceqq_u8(bytes, vdupq_n_u8(c)), powers);
                    (u64::from(vaddv_u8(vget_low_u8(bits)))
                        | (u64::from(vaddv_u8(vget_high_u8(bits))) << 8))
                        << j
                };
                m.separators |= eq(b',');
                m.newlines |= eq(b'\n');
                m.quotes |= eq(b'"');
            }
        }
        m.separators |= m.newlines;
        m
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let mut m = Masks::default();
        for (i, &b) in bytes.iter().enumerate() {
            m.newlines |= u64::from(b == b'\n') << i;
            m.separators |= u64::from(matches!(b, b',' | b'\n')) << i;
            m.quotes |= u64::from(b == b'"') << i;
        }
        m
    }
}
const TILE: usize = 4096;
fn index<const N: usize>(tile: &[u8], ends: &mut [u16; TILE + 1]) -> Option<usize> {
    let (mut n, mut rows, mut errors, mut carry, mut prev) = (0, 0, 0u64, 0u64, 1);
    for (k, chunk) in tile.chunks(64).enumerate() {
        let base = k * 64;
        let mut tail = [0; 64];
        let block = if let Ok(block) = chunk.try_into() {
            block
        } else {
            tail[..chunk.len()].copy_from_slice(chunk);
            &tail
        };
        let m = masks(block);
        let mut parity = m.quotes;
        for shift in [1, 2, 4, 8, 16, 32] {
            parity ^= parity << shift;
        }
        let inside = parity ^ carry;
        let next = u64::from(matches!(tile.get(base + 64), Some(b',' | b'\n')));
        let preceded = m.separators << 1 | prev;
        let followed = m.separators >> 1 | next << 63;
        errors |= (m.quotes & inside & !preceded)
            | (m.quotes & !inside & !followed)
            | (m.separators & inside);
        carry = 0u64.wrapping_sub(inside >> 63);
        prev = m.separators >> 63;
        rows += m.newlines.count_ones() as usize;
        let mut separators = m.separators;
        while separators != 0 {
            n += 1;
            ends[n] = (base + separators.trailing_zeros() as usize + 1) as u16;
            separators &= separators - 1;
        }
    }
    if errors != 0 || n != N * rows {
        return None;
    }
    // Counting separators alone cannot detect a short row followed by a long one.
    (N..=n)
        .step_by(N)
        .all(|i| tile[usize::from(ends[i]) - 1] == b'\n')
        .then_some(n)
}
pub fn rows<'a, const N: usize>(mut block: &'a [u8], mut consume: impl FnMut([&'a [u8]; N])) {
    let mut ends = [0; TILE + 1];
    while !block.is_empty() {
        let prefix = TILE.min(block.len());
        let Some(lf) = memrchr(b'\n', &block[..prefix]) else {
            let end = memchr(b'\n', &block[prefix..]).map_or(block.len(), |j| prefix + j);
            if let Some(f) = fields(&block[..end]) {
                consume(f);
            }
            block = &block[(end + 1).min(block.len())..];
            continue;
        };
        let tile = &block[..=lf];
        block = &block[lf + 1..];
        if let Some(count) = index::<N>(tile, &mut ends) {
            for row in (0..count).step_by(N) {
                let f = std::array::from_fn(|col| {
                    let start = usize::from(ends[row + col]);
                    let end = usize::from(ends[row + col + 1]) - 1;
                    let quote = usize::from(tile[start] == b'"');
                    &tile[start + quote..end - quote]
                });
                consume(f);
            }
        } else {
            lines(tile, |line| {
                if let Some(f) = fields(line) {
                    consume(f);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Exercise the fast tile, malformed fallback, 64-byte quote carry, and large rows.
    #[test]
    fn agrees_with_scalar() {
        let mut data = Vec::new();
        for pad in 0..128 {
            for row in [
                b",\"\",ok\n".as_slice(),
                b",\"a\",b\n",
                b",a\"b,c\n",
                b",\"x,y\"\n",
                b",,\n",
                b",a,b,too\n",
            ] {
                data.extend(std::iter::repeat_n(b' ', pad));
                data.extend_from_slice(row);
            }
        }
        data.extend(std::iter::repeat_n(b'x', 5000));
        data.extend_from_slice(b",b,c\nlast,b,c");
        fn compare(data: &[u8]) {
            let mut got = vec![];
            let mut expected = vec![];
            rows::<3>(data, |f| got.push(f));
            lines(data, |l| {
                if let Some(f) = fields(l) {
                    expected.push(f);
                }
            });
            assert_eq!(got, expected);
        }
        compare(&data);
        let clean = b"\"ab\",c,\"d\"\nq,,\" \"\n".repeat(2000);
        compare(&clean);
    }
}
