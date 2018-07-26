//! Utility functions for utf8 string slices.
//!
//! This module provides various utility functions that operate on string
//! slices in ways compatible with Ropey.  They may be useful when building
//! additional functionality on top of Ropey.

use std;
use std::arch::x86_64;

/// Converts from byte-index to char-index in a string slice.
///
/// If the byte is in the middle of a multi-byte char, returns the index of
/// the char that the byte belongs to.
///
/// Any past-the-end index will return the one-past-the-end char index.
#[inline]
pub fn byte_to_char_idx(text: &str, byte_idx: usize) -> usize {
    if byte_idx == 0 {
        return 0;
    } else if byte_idx >= text.len() {
        return count_chars(text);
    } else {
        return count_chars(unsafe {
            std::str::from_utf8_unchecked(&text.as_bytes()[0..(byte_idx + 1)])
        }) - 1;
    }
}

/// Converts from byte-index to line-index in a string slice.
///
/// This is equivalent to counting the line endings before the given byte.
///
/// Any past-the-end index will return the last line index.
#[inline]
pub fn byte_to_line_idx(text: &str, byte_idx: usize) -> usize {
    use crlf;
    let mut byte_idx = byte_idx.min(text.len());
    while !text.is_char_boundary(byte_idx) {
        byte_idx -= 1;
    }
    let nl_count = count_line_breaks(&text[..byte_idx]);
    if crlf::is_break(byte_idx, text.as_bytes()) {
        nl_count
    } else {
        nl_count - 1
    }
}

/// Converts from char-index to byte-index in a string slice.
///
/// Any past-the-end index will return the one-past-the-end byte index.
#[inline]
pub fn char_to_byte_idx(text: &str, char_idx: usize) -> usize {
    if is_x86_feature_detected!("avx2") {
        char_to_byte_idx_inner::<x86_64::__m256i>(text, char_idx)
    } else if is_x86_feature_detected!("sse2") {
        char_to_byte_idx_inner::<x86_64::__m128i>(text, char_idx)
    } else {
        char_to_byte_idx_inner::<usize>(text, char_idx)
    }
}

#[inline(always)]
fn char_to_byte_idx_inner<T: ByteChunk>(text: &str, char_idx: usize) -> usize {
    let mut char_count = 0;
    let mut ptr = text.as_ptr();
    let start_ptr = text.as_ptr();
    let end_ptr = unsafe { ptr.offset(text.len() as isize) };

    // Take care of any unaligned bytes at the beginning
    let end_pre_ptr = {
        let aligned = ptr as usize + (T::size() - (ptr as usize & (T::size() - 1)));
        (end_ptr as usize).min(aligned) as *const u8
    };
    while ptr < end_pre_ptr && char_count <= char_idx {
        let byte = unsafe { *ptr };
        char_count += ((byte & 0xC0) != 0x80) as usize;
        ptr = unsafe { ptr.offset(1) };
    }

    // Use chunks to count multiple bytes at once, using bit-fiddling magic.
    let mut ptr = ptr as *const T;
    let end_mid_ptr = (end_ptr as usize - (end_ptr as usize & (T::size() - 1))) as *const T;
    let mut acc = T::splat(0);
    let mut i = 0;
    while ptr < end_mid_ptr && (char_count + (T::size() * (i + 1))) <= char_idx {
        // Do the clever counting
        let n = unsafe { *ptr };
        let tmp = n.bitand(T::splat(0xc0)).cmp_eq_byte(0x80);
        acc = acc.add(tmp);
        i += 1;
        if i == T::max_acc() || (char_count + (T::size() * (i + 1))) > char_idx {
            char_count += (T::size() * i) - acc.sum_bytes();
            i = 0;
            acc = T::splat(0);
        }
        ptr = unsafe { ptr.offset(1) };
    }
    char_count += (T::size() * i) - acc.sum_bytes();

    // Take care of any unaligned bytes at the end
    let mut ptr = ptr as *const u8;
    while ptr < end_ptr && char_count <= char_idx {
        let byte = unsafe { *ptr };
        char_count += ((byte & 0xC0) != 0x80) as usize;
        ptr = unsafe { ptr.offset(1) };
    }

    // Finish up
    let byte_count = ptr as usize - start_ptr as usize;
    if ptr == end_ptr && char_count <= char_idx {
        byte_count
    } else {
        byte_count - 1
    }
}

/// Converts from char-index to line-index in a string slice.
///
/// This is equivalent to counting the line endings before the given char.
///
/// Any past-the-end index will return the last line index.
#[inline]
pub fn char_to_line_idx(text: &str, char_idx: usize) -> usize {
    byte_to_line_idx(text, char_to_byte_idx(text, char_idx))
}

/// Converts from line-index to byte-index in a string slice.
///
/// More specifically, this returns the index of the first byte of the given
/// line.
///
/// Any past-the-end index will return the one-past-the-end byte index.
#[inline]
pub fn line_to_byte_idx(text: &str, line_idx: usize) -> usize {
    if is_x86_feature_detected!("avx2") {
        line_to_byte_idx_inner::<x86_64::__m256i>(text, line_idx)
    } else if is_x86_feature_detected!("sse2") {
        line_to_byte_idx_inner::<x86_64::__m128i>(text, line_idx)
    } else {
        line_to_byte_idx_inner::<usize>(text, line_idx)
    }
}

#[inline(always)]
fn line_to_byte_idx_inner<T: ByteChunk>(text: &str, line_idx: usize) -> usize {
    let start_ptr = text.as_ptr();
    let end_ptr = unsafe { start_ptr.offset(text.len() as isize) };

    let mut line_break_count = 0;
    let mut ptr = start_ptr;
    while ptr < end_ptr && line_break_count < line_idx {
        // Count line breaks in big chunks.
        if ptr == align_ptr(ptr, T::size()) {
            while unsafe { ptr.offset(T::size() as isize) } < end_ptr {
                let tmp =
                    unsafe { count_line_breaks_in_chunks_from_ptr::<T>(ptr, end_ptr) }.sum_bytes();
                if tmp + line_break_count >= line_idx {
                    break;
                }
                line_break_count += tmp;

                ptr = unsafe { ptr.offset(T::size() as isize) };
            }
        }

        // Count line breaks a byte at a time.
        let end_aligned_ptr = next_aligned_ptr(ptr, T::size()).min(end_ptr);
        while ptr < end_aligned_ptr && line_break_count < line_idx {
            let byte = unsafe { *ptr };

            // Handle u{000A}, u{000B}, u{000C}, and u{000D}
            if (byte <= 0x0D) && (byte >= 0x0A) {
                line_break_count += 1;

                // Check for CRLF and and subtract 1 if it is,
                // since it will be caught in the next iteration
                // with the LF.
                if byte == 0x0D {
                    let next = unsafe { ptr.offset(1) };
                    if next < end_ptr && unsafe { *next } == 0x0A {
                        line_break_count -= 1;
                    }
                }
            }
            // Handle u{0085}
            else if byte == 0xC2 {
                let next = unsafe { ptr.offset(1) };
                if next < end_ptr && unsafe { *next } == 0x85 {
                    line_break_count += 1;
                }
            }
            // Handle u{2028} and u{2029}
            else if byte == 0xE2 {
                let next1 = unsafe { ptr.offset(1) };
                let next2 = unsafe { ptr.offset(2) };
                if next1 < end_ptr
                    && next2 < end_ptr
                    && unsafe { *next1 } == 0x80
                    && (unsafe { *next2 } >> 1) == 0x54
                {
                    line_break_count += 1;
                }
            }

            ptr = unsafe { ptr.offset(1) };
        }
    }

    // Finish up
    let mut byte_idx = ptr as usize - start_ptr as usize;
    while !text.is_char_boundary(byte_idx) {
        byte_idx += 1;
    }
    byte_idx
}

/// Converts from line-index to char-index in a string slice.
///
/// More specifically, this returns the index of the first char of the given
/// line.
///
/// Any past-the-end index will return the one-past-the-end char index.
#[inline]
pub fn line_to_char_idx(text: &str, line_idx: usize) -> usize {
    byte_to_char_idx(text, line_to_byte_idx(text, line_idx))
}

//===========================================================================
// Internal
//===========================================================================

/// Uses bit-fiddling magic to count utf8 chars really quickly.
/// We actually count the number of non-starting utf8 bytes, since
/// they have a consistent starting two-bit pattern.  We then
/// subtract from the byte length of the text to get the final
/// count.
#[inline]
pub(crate) fn count_chars(text: &str) -> usize {
    if is_x86_feature_detected!("avx2") {
        count_chars_internal::<x86_64::__m256i>(text)
    } else if is_x86_feature_detected!("sse2") {
        count_chars_internal::<x86_64::__m128i>(text)
    } else {
        count_chars_internal::<usize>(text)
    }
}

#[inline(always)]
fn count_chars_internal<T: ByteChunk>(text: &str) -> usize {
    let len = text.len();
    let mut ptr = text.as_ptr();
    let end_ptr = unsafe { ptr.offset(len as isize) };
    let mut inv_count = 0;

    // Take care of any unaligned bytes at the beginning
    let end_pre_ptr = align_ptr(ptr, T::size()).min(end_ptr);
    while ptr < end_pre_ptr {
        let byte = unsafe { *ptr };
        inv_count += ((byte & 0xC0) == 0x80) as usize;
        ptr = unsafe { ptr.offset(1) };
    }

    // Use chunks to count multiple bytes at once.
    let mut ptr = ptr as *const T;
    let end_mid_ptr = (end_ptr as usize - (end_ptr as usize & (T::size() - 1))) as *const T;
    let mut acc = T::splat(0);
    let mut i = 0;
    while ptr < end_mid_ptr {
        // Do the clever counting
        let n = unsafe { *ptr };
        let tmp = n.bitand(T::splat(0xc0)).cmp_eq_byte(0x80);
        acc = acc.add(tmp);
        i += 1;
        if i == T::max_acc() {
            i = 0;
            inv_count += acc.sum_bytes();
            acc = T::splat(0);
        }
        ptr = unsafe { ptr.offset(1) };
    }
    inv_count += acc.sum_bytes();

    // Take care of any unaligned bytes at the end
    let mut ptr = ptr as *const u8;
    while ptr < end_ptr {
        let byte = unsafe { *ptr };
        inv_count += ((byte & 0xC0) == 0x80) as usize;
        ptr = unsafe { ptr.offset(1) };
    }

    len - inv_count
}

/// Uses bit-fiddling magic to count line breaks really quickly.
///
/// The following unicode sequences are considered newlines by this function:
/// - u{000A}        (Line Feed)
/// - u{000B}        (Vertical Tab)
/// - u{000C}        (Form Feed)
/// - u{000D}        (Carriage Return)
/// - u{000D}u{000A} (Carriage Return + Line Feed)
/// - u{0085}        (Next Line)
/// - u{2028}        (Line Separator)
/// - u{2029}        (Paragraph Separator)
#[inline]
pub(crate) fn count_line_breaks(text: &str) -> usize {
    if is_x86_feature_detected!("avx2") {
        count_line_breaks_internal::<x86_64::__m256i>(text)
    } else if is_x86_feature_detected!("sse2") {
        count_line_breaks_internal::<x86_64::__m128i>(text)
    } else {
        count_line_breaks_internal::<usize>(text)
    }
}

#[inline(always)]
fn count_line_breaks_internal<T: ByteChunk>(text: &str) -> usize {
    let len = text.len();
    let mut ptr = text.as_ptr();
    let end_ptr = unsafe { ptr.offset(len as isize) };
    let mut count = 0;

    while ptr < end_ptr {
        // Count line breaks in big chunks.
        if ptr == align_ptr(ptr, T::size()) {
            let mut i = 0;
            let mut acc = T::splat(0);
            while unsafe { ptr.offset(T::size() as isize) } < end_ptr {
                acc = acc.add(unsafe { count_line_breaks_in_chunks_from_ptr::<T>(ptr, end_ptr) });
                ptr = unsafe { ptr.offset(T::size() as isize) };
                i += 1;
                if i == T::max_acc() {
                    i = 0;
                    count += acc.sum_bytes();
                    acc = T::splat(0);
                }
            }
            count += acc.sum_bytes();
        }

        // Count line breaks a byte at a time.
        let end_aligned_ptr = next_aligned_ptr(ptr, T::size()).min(end_ptr);
        while ptr < end_aligned_ptr {
            let byte = unsafe { *ptr };

            // Handle u{000A}, u{000B}, u{000C}, and u{000D}
            if (byte <= 0x0D) && (byte >= 0x0A) {
                count += 1;

                // Check for CRLF and and subtract 1 if it is,
                // since it will be caught in the next iteration
                // with the LF.
                if byte == 0x0D {
                    let next = unsafe { ptr.offset(1) };
                    if next < end_ptr && unsafe { *next } == 0x0A {
                        count -= 1;
                    }
                }
            }
            // Handle u{0085}
            else if byte == 0xC2 {
                let next = unsafe { ptr.offset(1) };
                if next < end_ptr && unsafe { *next } == 0x85 {
                    count += 1;
                }
            }
            // Handle u{2028} and u{2029}
            else if byte == 0xE2 {
                let next1 = unsafe { ptr.offset(1) };
                let next2 = unsafe { ptr.offset(2) };
                if next1 < end_ptr
                    && next2 < end_ptr
                    && unsafe { *next1 } == 0x80
                    && (unsafe { *next2 } >> 1) == 0x54
                {
                    count += 1;
                }
            }

            ptr = unsafe { ptr.offset(1) };
        }
    }

    count
}

/// Used internally in the line-break counting functions.
///
/// ptr MUST be aligned to T alignment.
#[inline(always)]
unsafe fn count_line_breaks_in_chunks_from_ptr<T: ByteChunk>(
    ptr: *const u8,
    end_ptr: *const u8,
) -> T {
    let mut acc = T::splat(0);
    let c = *(ptr as *const T);
    let next_ptr = ptr.offset(T::size() as isize);

    // Calculate the flags we're going to be working with.
    let nl_1_flags = c.cmp_eq_byte(0xC2);
    let sp_1_flags = c.cmp_eq_byte(0xE2);
    let all_flags = c.bytes_between(0x09, 0x0E);
    let cr_flags = c.cmp_eq_byte(0x0D);

    // Next Line: u{0085}
    if !nl_1_flags.is_zero() {
        let nl_2_flags = c.cmp_eq_byte(0x85).shift_back_lex(1);
        let flags = nl_1_flags.bitand(nl_2_flags);
        acc = acc.add(flags);

        // Handle ending boundary
        if next_ptr < end_ptr && *next_ptr.offset(-1) == 0xC2 && *next_ptr == 0x85 {
            acc = acc.inc_nth_from_end_lex_byte(0);
        }
    }

    // Line Separator:      u{2028}
    // Paragraph Separator: u{2029}
    if !sp_1_flags.is_zero() {
        let sp_2_flags = c.cmp_eq_byte(0x80).shift_back_lex(1).bitand(sp_1_flags);
        if !sp_2_flags.is_zero() {
            let sp_3_flags = c.shr(1)
                .bitand(T::splat(!0x80))
                .cmp_eq_byte(0x54)
                .shift_back_lex(2);
            let sp_flags = sp_2_flags.bitand(sp_3_flags);
            acc = acc.add(sp_flags);
        }

        // Handle ending boundary
        if next_ptr < end_ptr
            && *next_ptr.offset(-2) == 0xE2
            && *next_ptr.offset(-1) == 0x80
            && (*next_ptr >> 1) == 0x54
        {
            acc = acc.inc_nth_from_end_lex_byte(1);
        } else if next_ptr.offset(1) < end_ptr
            && *next_ptr.offset(-1) == 0xE2
            && *next_ptr == 0x80
            && (*next_ptr.offset(1) >> 1) == 0x54
        {
            acc = acc.inc_nth_from_end_lex_byte(0);
        }
    }

    // Line Feed:                   u{000A}
    // Vertical Tab:                u{000B}
    // Form Feed:                   u{000C}
    // Carriage Return:             u{000D}
    // Carriage Return + Line Feed: u{000D}u{000A}
    acc = acc.add(all_flags);
    if !cr_flags.is_zero() {
        // Handle CRLF
        let lf_flags = c.cmp_eq_byte(0x0A);
        let crlf_flags = cr_flags.bitand(lf_flags.shift_back_lex(1));
        acc = acc.sub(crlf_flags);
        if next_ptr < end_ptr && *next_ptr.offset(-1) == 0x0D && *next_ptr == 0x0A {
            acc = acc.dec_last_lex_byte();
        }
    }

    acc
}

/// Returns the next pointer after `ptr` that is aligned with `alignment`.
///
/// NOTE: only works for power-of-two alignments.
#[inline(always)]
fn next_aligned_ptr<T>(ptr: *const T, alignment: usize) -> *const T {
    (ptr as usize + alignment - (ptr as usize & (alignment - 1))) as *const T
}

/// Returns `ptr` if aligned to `alignment`, or the next aligned pointer
/// after if not.
///
/// NOTE: only works for power-of-two alignments.
#[inline(always)]
fn align_ptr<T>(ptr: *const T, alignment: usize) -> *const T {
    next_aligned_ptr(unsafe { ptr.offset(-1) }, alignment)
}

//======================================================================

trait ByteChunk: Copy + Clone {
    /// Returns the size of the chunk in bytes.
    fn size() -> usize;

    /// Returns the maximum number of iterations the chunk can accumulate
    /// before sum_bytes() becomes inaccurate.
    fn max_acc() -> usize;

    /// Creates a new chunk with all bytes set to n.
    fn splat(n: u8) -> Self;

    /// Returns whether all bytes are zero or not.
    fn is_zero(&self) -> bool;

    /// Shifts bytes back lexographically by n bytes.
    fn shift_back_lex(&self, n: usize) -> Self;

    /// Shifts bits to the right by n bits.
    fn shr(&self, n: usize) -> Self;

    /// Compares bytes for equality with the given byte.
    ///
    /// Bytes that are equal are set to 1, bytes that are not
    /// are set to 0.
    fn cmp_eq_byte(&self, byte: u8) -> Self;

    /// Returns true if any of the bytes in the chunk are < n.
    fn has_bytes_less_than(&self, n: u8) -> bool;

    /// Compares bytes to see if they're in the non-inclusive range (a, b).
    ///
    /// Bytes in the range are set to 1, bytes not in the range are set to 0.
    fn bytes_between(&self, a: u8, b: u8) -> Self;

    /// Performs a bitwise and on two chunks.
    fn bitand(&self, other: Self) -> Self;

    /// Adds the bytes of two chunks together.
    fn add(&self, other: Self) -> Self;

    /// Subtracts other's bytes from this chunk.
    fn sub(&self, other: Self) -> Self;

    /// Increments the nth-from-last lexographic byte by 1.
    fn inc_nth_from_end_lex_byte(&self, n: usize) -> Self;

    /// Decrements the last lexographic byte by 1.
    fn dec_last_lex_byte(&self) -> Self;

    /// Returns the sum of all bytes in the chunk.
    fn sum_bytes(&self) -> usize;
}

impl ByteChunk for usize {
    #[inline(always)]
    fn size() -> usize {
        std::mem::size_of::<usize>()
    }

    #[inline(always)]
    fn max_acc() -> usize {
        (256 / std::mem::size_of::<usize>()) - 1
    }

    #[inline(always)]
    fn splat(n: u8) -> Self {
        const ONES: usize = std::usize::MAX / 0xFF;
        ONES * n as usize
    }

    #[inline(always)]
    fn is_zero(&self) -> bool {
        *self == 0
    }

    #[inline(always)]
    fn shift_back_lex(&self, n: usize) -> Self {
        if cfg!(target_endian = "little") {
            *self >> (n * 8)
        } else {
            *self << (n * 8)
        }
    }

    #[inline(always)]
    fn shr(&self, n: usize) -> Self {
        *self >> n
    }

    #[inline(always)]
    fn cmp_eq_byte(&self, byte: u8) -> Self {
        const ONES: usize = std::usize::MAX / 0xFF;
        const ONES_HIGH: usize = ONES << 7;
        let word = *self ^ (byte as usize * ONES);
        (!(((word & !ONES_HIGH) + !ONES_HIGH) | word) & ONES_HIGH) >> 7
    }

    #[inline(always)]
    fn has_bytes_less_than(&self, n: u8) -> bool {
        const ONES: usize = std::usize::MAX / 0xFF;
        const ONES_HIGH: usize = ONES << 7;
        ((self.wrapping_sub(ONES * n as usize)) & !*self & ONES_HIGH) != 0
    }

    #[inline(always)]
    fn bytes_between(&self, a: u8, b: u8) -> Self {
        const ONES: usize = std::usize::MAX / 0xFF;
        const ONES_HIGH: usize = ONES << 7;
        let tmp = *self & (ONES * 127);
        ((ONES * (127 + b as usize) - tmp & !*self & tmp + (ONES * (127 - a as usize))) & ONES_HIGH)
            >> 7
    }

    #[inline(always)]
    fn bitand(&self, other: Self) -> Self {
        *self & other
    }

    #[inline(always)]
    fn add(&self, other: Self) -> Self {
        *self + other
    }

    #[inline(always)]
    fn sub(&self, other: Self) -> Self {
        *self - other
    }

    #[inline(always)]
    fn inc_nth_from_end_lex_byte(&self, n: usize) -> Self {
        if cfg!(target_endian = "little") {
            *self + (1 << ((Self::size() - 1 - n) * 8))
        } else {
            *self + (1 << (n * 8))
        }
    }

    #[inline(always)]
    fn dec_last_lex_byte(&self) -> Self {
        if cfg!(target_endian = "little") {
            *self - (1 << ((Self::size() - 1) * 8))
        } else {
            *self - 1
        }
    }

    #[inline(always)]
    fn sum_bytes(&self) -> usize {
        const ONES: usize = std::usize::MAX / 0xFF;
        self.wrapping_mul(ONES) >> ((Self::size() - 1) * 8)
    }
}

impl ByteChunk for x86_64::__m128i {
    #[inline(always)]
    fn size() -> usize {
        std::mem::size_of::<x86_64::__m128i>()
    }

    #[inline(always)]
    fn max_acc() -> usize {
        (256 / 8) - 1
    }

    #[inline(always)]
    fn splat(n: u8) -> Self {
        unsafe { x86_64::_mm_set1_epi8(n as i8) }
    }

    #[inline(always)]
    fn is_zero(&self) -> bool {
        let tmp = unsafe { std::mem::transmute::<Self, (u64, u64)>(*self) };
        tmp.0 == 0 && tmp.1 == 0
    }

    #[inline(always)]
    fn shift_back_lex(&self, n: usize) -> Self {
        match n {
            0 => *self,
            1 => unsafe { x86_64::_mm_srli_si128(*self, 1) },
            2 => unsafe { x86_64::_mm_srli_si128(*self, 2) },
            3 => unsafe { x86_64::_mm_srli_si128(*self, 3) },
            4 => unsafe { x86_64::_mm_srli_si128(*self, 4) },
            _ => unreachable!(),
        }
    }

    #[inline(always)]
    fn shr(&self, n: usize) -> Self {
        match n {
            0 => *self,
            1 => unsafe { x86_64::_mm_srli_epi64(*self, 1) },
            2 => unsafe { x86_64::_mm_srli_epi64(*self, 2) },
            3 => unsafe { x86_64::_mm_srli_epi64(*self, 3) },
            4 => unsafe { x86_64::_mm_srli_epi64(*self, 4) },
            _ => unreachable!(),
        }
    }

    #[inline(always)]
    fn cmp_eq_byte(&self, byte: u8) -> Self {
        let tmp = unsafe { x86_64::_mm_cmpeq_epi8(*self, Self::splat(byte)) };
        unsafe { x86_64::_mm_and_si128(tmp, Self::splat(1)) }
    }

    #[inline(always)]
    fn has_bytes_less_than(&self, n: u8) -> bool {
        let tmp = unsafe { x86_64::_mm_cmplt_epi8(*self, Self::splat(n)) };
        !tmp.is_zero()
    }

    #[inline(always)]
    fn bytes_between(&self, a: u8, b: u8) -> Self {
        let tmp1 = unsafe { x86_64::_mm_cmpgt_epi8(*self, Self::splat(a)) };
        let tmp2 = unsafe { x86_64::_mm_cmplt_epi8(*self, Self::splat(b)) };
        let tmp3 = unsafe { x86_64::_mm_and_si128(tmp1, tmp2) };
        unsafe { x86_64::_mm_and_si128(tmp3, Self::splat(1)) }
    }

    #[inline(always)]
    fn bitand(&self, other: Self) -> Self {
        unsafe { x86_64::_mm_and_si128(*self, other) }
    }

    #[inline(always)]
    fn add(&self, other: Self) -> Self {
        unsafe { x86_64::_mm_add_epi8(*self, other) }
    }

    #[inline(always)]
    fn sub(&self, other: Self) -> Self {
        unsafe { x86_64::_mm_sub_epi8(*self, other) }
    }

    #[inline(always)]
    fn inc_nth_from_end_lex_byte(&self, n: usize) -> Self {
        let mut tmp = unsafe { std::mem::transmute::<Self, [u8; 16]>(*self) };
        tmp[15 - n] += 1;
        unsafe { std::mem::transmute::<[u8; 16], Self>(tmp) }
    }

    #[inline(always)]
    fn dec_last_lex_byte(&self) -> Self {
        let mut tmp = unsafe { std::mem::transmute::<Self, [u8; 16]>(*self) };
        tmp[15] -= 1;
        unsafe { std::mem::transmute::<[u8; 16], Self>(tmp) }
    }

    #[inline(always)]
    fn sum_bytes(&self) -> usize {
        const ONES: u64 = std::u64::MAX / 0xFF;
        let tmp = unsafe { std::mem::transmute::<Self, (u64, u64)>(*self) };
        let a = tmp.0.wrapping_mul(ONES) >> (7 * 8);
        let b = tmp.1.wrapping_mul(ONES) >> (7 * 8);
        (a + b) as usize
    }
}

impl ByteChunk for x86_64::__m256i {
    #[inline(always)]
    fn size() -> usize {
        std::mem::size_of::<x86_64::__m256i>()
    }

    #[inline(always)]
    fn max_acc() -> usize {
        (256 / 8) - 1
    }

    #[inline(always)]
    fn splat(n: u8) -> Self {
        unsafe { x86_64::_mm256_set1_epi8(n as i8) }
    }

    #[inline(always)]
    fn is_zero(&self) -> bool {
        let tmp = unsafe { std::mem::transmute::<Self, (u64, u64, u64, u64)>(*self) };
        tmp.0 == 0 && tmp.1 == 0 && tmp.2 == 0 && tmp.3 == 0
    }

    #[inline(always)]
    fn shift_back_lex(&self, n: usize) -> Self {
        let mut tmp1;
        let tmp2 = unsafe { std::mem::transmute::<Self, [u8; 32]>(*self) };
        match n {
            0 => return *self,
            1 => {
                tmp1 = unsafe {
                    std::mem::transmute::<Self, [u8; 32]>(x86_64::_mm256_srli_si256(*self, 1))
                };
                tmp1[15] = tmp2[16];
            }
            2 => {
                tmp1 = unsafe {
                    std::mem::transmute::<Self, [u8; 32]>(x86_64::_mm256_srli_si256(*self, 2))
                };
                tmp1[15] = tmp2[17];
                tmp1[14] = tmp2[16];
            }
            _ => unreachable!(),
        }
        unsafe { std::mem::transmute::<[u8; 32], Self>(tmp1) }
    }

    #[inline(always)]
    fn shr(&self, n: usize) -> Self {
        match n {
            0 => *self,
            1 => unsafe { x86_64::_mm256_srli_epi64(*self, 1) },
            2 => unsafe { x86_64::_mm256_srli_epi64(*self, 2) },
            3 => unsafe { x86_64::_mm256_srli_epi64(*self, 3) },
            4 => unsafe { x86_64::_mm256_srli_epi64(*self, 4) },
            _ => unreachable!(),
        }
    }

    #[inline(always)]
    fn cmp_eq_byte(&self, byte: u8) -> Self {
        let tmp = unsafe { x86_64::_mm256_cmpeq_epi8(*self, Self::splat(byte)) };
        unsafe { x86_64::_mm256_and_si256(tmp, Self::splat(1)) }
    }

    #[inline(always)]
    fn has_bytes_less_than(&self, n: u8) -> bool {
        let tmp1 = unsafe { x86_64::_mm256_cmpgt_epi8(*self, Self::splat(n + 1)) };
        let tmp2 = unsafe { x86_64::_mm256_andnot_si256(tmp1, Self::splat(0xff)) };
        !tmp2.is_zero()
    }

    #[inline(always)]
    fn bytes_between(&self, a: u8, b: u8) -> Self {
        let tmp2 = unsafe { x86_64::_mm256_cmpgt_epi8(*self, Self::splat(a)) };
        let tmp1 = {
            let tmp = unsafe { x86_64::_mm256_cmpgt_epi8(*self, Self::splat(b + 1)) };
            unsafe { x86_64::_mm256_andnot_si256(tmp, Self::splat(0xff)) }
        };
        let tmp3 = unsafe { x86_64::_mm256_and_si256(tmp1, tmp2) };
        unsafe { x86_64::_mm256_and_si256(tmp3, Self::splat(1)) }
    }

    #[inline(always)]
    fn bitand(&self, other: Self) -> Self {
        unsafe { x86_64::_mm256_and_si256(*self, other) }
    }

    #[inline(always)]
    fn add(&self, other: Self) -> Self {
        unsafe { x86_64::_mm256_add_epi8(*self, other) }
    }

    #[inline(always)]
    fn sub(&self, other: Self) -> Self {
        unsafe { x86_64::_mm256_sub_epi8(*self, other) }
    }

    #[inline(always)]
    fn inc_nth_from_end_lex_byte(&self, n: usize) -> Self {
        let mut tmp = unsafe { std::mem::transmute::<Self, [u8; 32]>(*self) };
        tmp[31 - n] += 1;
        unsafe { std::mem::transmute::<[u8; 32], Self>(tmp) }
    }

    #[inline(always)]
    fn dec_last_lex_byte(&self) -> Self {
        let mut tmp = unsafe { std::mem::transmute::<Self, [u8; 32]>(*self) };
        tmp[31] -= 1;
        unsafe { std::mem::transmute::<[u8; 32], Self>(tmp) }
    }

    #[inline(always)]
    fn sum_bytes(&self) -> usize {
        const ONES: u64 = std::u64::MAX / 0xFF;
        let tmp = unsafe { std::mem::transmute::<Self, (u64, u64, u64, u64)>(*self) };
        let a = tmp.0.wrapping_mul(ONES) >> (7 * 8);
        let b = tmp.1.wrapping_mul(ONES) >> (7 * 8);
        let c = tmp.2.wrapping_mul(ONES) >> (7 * 8);
        let d = tmp.3.wrapping_mul(ONES) >> (7 * 8);
        (a + b + c + d) as usize
    }
}

//======================================================================

/// An iterator that yields the byte indices of line breaks in a string.
/// A line break in this case is the point immediately *after* a newline
/// character.
///
/// The following unicode sequences are considered newlines by this function:
/// - u{000A}        (Line Feed)
/// - u{000B}        (Vertical Tab)
/// - u{000C}        (Form Feed)
/// - u{000D}        (Carriage Return)
/// - u{000D}u{000A} (Carriage Return + Line Feed)
/// - u{0085}        (Next Line)
/// - u{2028}        (Line Separator)
/// - u{2029}        (Paragraph Separator)
#[allow(unused)] // Used in tests, as reference solution.
struct LineBreakIter<'a> {
    byte_itr: std::str::Bytes<'a>,
    byte_idx: usize,
}

#[allow(unused)]
impl<'a> LineBreakIter<'a> {
    #[inline]
    fn new(text: &str) -> LineBreakIter {
        LineBreakIter {
            byte_itr: text.bytes(),
            byte_idx: 0,
        }
    }
}

impl<'a> Iterator for LineBreakIter<'a> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        while let Some(byte) = self.byte_itr.next() {
            self.byte_idx += 1;
            // Handle u{000A}, u{000B}, u{000C}, and u{000D}
            if (byte <= 0x0D) && (byte >= 0x0A) {
                if byte == 0x0D {
                    // We're basically "peeking" here.
                    if let Some(0x0A) = self.byte_itr.clone().next() {
                        self.byte_itr.next();
                        self.byte_idx += 1;
                    }
                }
                return Some(self.byte_idx);
            }
            // Handle u{0085}
            else if byte == 0xC2 {
                self.byte_idx += 1;
                if let Some(0x85) = self.byte_itr.next() {
                    return Some(self.byte_idx);
                }
            }
            // Handle u{2028} and u{2029}
            else if byte == 0xE2 {
                self.byte_idx += 2;
                let byte2 = self.byte_itr.next().unwrap();
                let byte3 = self.byte_itr.next().unwrap() >> 1;
                if byte2 == 0x80 && byte3 == 0x54 {
                    return Some(self.byte_idx);
                }
            }
        }

        return None;
    }
}

//======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // 124 bytes, 100 chars, 4 lines
    const TEXT_LINES: &str = "Hello there!  How're you doing?\nIt's \
                              a fine day, isn't it?\nAren't you glad \
                              we're alive?\nこんにちは、みんなさん！";

    #[test]
    fn count_chars_01() {
        let text =
            "Hello せかい! Hello せかい! Hello せかい! Hello せかい! Hello せかい!";

        assert_eq!(54, count_chars(text));
    }

    #[test]
    fn count_chars_02() {
        assert_eq!(100, count_chars(TEXT_LINES));
    }

    #[test]
    fn line_breaks_iter_01() {
        let text = "\u{000A}Hello\u{000D}\u{000A}\u{000D}せ\u{000B}か\u{000C}い\u{0085}. \
                    There\u{2028}is something.\u{2029}";
        let mut itr = LineBreakIter::new(text);
        assert_eq!(48, text.len());
        assert_eq!(Some(1), itr.next());
        assert_eq!(Some(8), itr.next());
        assert_eq!(Some(9), itr.next());
        assert_eq!(Some(13), itr.next());
        assert_eq!(Some(17), itr.next());
        assert_eq!(Some(22), itr.next());
        assert_eq!(Some(32), itr.next());
        assert_eq!(Some(48), itr.next());
        assert_eq!(None, itr.next());
    }

    #[test]
    fn count_line_breaks_01() {
        let text = "\u{000A}Hello\u{000D}\u{000A}\u{000D}せ\u{000B}か\u{000C}い\u{0085}. \
                    There\u{2028}is something.\u{2029}";
        assert_eq!(48, text.len());
        assert_eq!(8, count_line_breaks(text));
    }

    #[test]
    fn count_line_breaks_02() {
        let text = "\u{000A}Hello world!  This is a longer text.\u{000D}\u{000A}\u{000D}To better test that skipping by usize doesn't mess things up.\u{000B}Hello せかい!\u{000C}\u{0085}Yet more text.  How boring.\u{2028}Hi.\u{2029}\u{000A}Hello world!  This is a longer text.\u{000D}\u{000A}\u{000D}To better test that skipping by usize doesn't mess things up.\u{000B}Hello せかい!\u{000C}\u{0085}Yet more text.  How boring.\u{2028}Hi.\u{2029}\u{000A}Hello world!  This is a longer text.\u{000D}\u{000A}\u{000D}To better test that skipping by usize doesn't mess things up.\u{000B}Hello せかい!\u{000C}\u{0085}Yet more text.  How boring.\u{2028}Hi.\u{2029}\u{000A}Hello world!  This is a longer text.\u{000D}\u{000A}\u{000D}To better test that skipping by usize doesn't mess things up.\u{000B}Hello せかい!\u{000C}\u{0085}Yet more text.  How boring.\u{2028}Hi.\u{2029}";
        assert_eq!(count_line_breaks(text), LineBreakIter::new(text).count());
    }

    #[test]
    fn byte_to_char_idx_01() {
        let text = "Hello せかい!";
        assert_eq!(0, byte_to_char_idx(text, 0));
        assert_eq!(1, byte_to_char_idx(text, 1));
        assert_eq!(6, byte_to_char_idx(text, 6));
        assert_eq!(6, byte_to_char_idx(text, 7));
        assert_eq!(6, byte_to_char_idx(text, 8));
        assert_eq!(7, byte_to_char_idx(text, 9));
        assert_eq!(7, byte_to_char_idx(text, 10));
        assert_eq!(7, byte_to_char_idx(text, 11));
        assert_eq!(8, byte_to_char_idx(text, 12));
        assert_eq!(8, byte_to_char_idx(text, 13));
        assert_eq!(8, byte_to_char_idx(text, 14));
        assert_eq!(9, byte_to_char_idx(text, 15));
        assert_eq!(10, byte_to_char_idx(text, 16));
        assert_eq!(10, byte_to_char_idx(text, 17));
        assert_eq!(10, byte_to_char_idx(text, 18));
        assert_eq!(10, byte_to_char_idx(text, 19));
    }

    #[test]
    fn byte_to_char_idx_02() {
        let text = "せかい";
        assert_eq!(0, byte_to_char_idx(text, 0));
        assert_eq!(0, byte_to_char_idx(text, 1));
        assert_eq!(0, byte_to_char_idx(text, 2));
        assert_eq!(1, byte_to_char_idx(text, 3));
        assert_eq!(1, byte_to_char_idx(text, 4));
        assert_eq!(1, byte_to_char_idx(text, 5));
        assert_eq!(2, byte_to_char_idx(text, 6));
        assert_eq!(2, byte_to_char_idx(text, 7));
        assert_eq!(2, byte_to_char_idx(text, 8));
        assert_eq!(3, byte_to_char_idx(text, 9));
        assert_eq!(3, byte_to_char_idx(text, 10));
        assert_eq!(3, byte_to_char_idx(text, 11));
        assert_eq!(3, byte_to_char_idx(text, 12));
    }

    #[test]
    fn byte_to_char_idx_03() {
        // Ascii range
        for i in 0..88 {
            assert_eq!(i, byte_to_char_idx(TEXT_LINES, i));
        }

        // Hiragana characters
        for i in 88..125 {
            assert_eq!(88 + ((i - 88) / 3), byte_to_char_idx(TEXT_LINES, i));
        }

        // Past the end
        for i in 125..130 {
            assert_eq!(100, byte_to_char_idx(TEXT_LINES, i));
        }
    }

    #[test]
    fn byte_to_line_idx_01() {
        let text = "Here\nare\nsome\nwords";
        assert_eq!(0, byte_to_line_idx(text, 0));
        assert_eq!(0, byte_to_line_idx(text, 4));
        assert_eq!(1, byte_to_line_idx(text, 5));
        assert_eq!(1, byte_to_line_idx(text, 8));
        assert_eq!(2, byte_to_line_idx(text, 9));
        assert_eq!(2, byte_to_line_idx(text, 13));
        assert_eq!(3, byte_to_line_idx(text, 14));
        assert_eq!(3, byte_to_line_idx(text, 19));
    }

    #[test]
    fn byte_to_line_idx_02() {
        let text = "\nHere\nare\nsome\nwords\n";
        assert_eq!(0, byte_to_line_idx(text, 0));
        assert_eq!(1, byte_to_line_idx(text, 1));
        assert_eq!(1, byte_to_line_idx(text, 5));
        assert_eq!(2, byte_to_line_idx(text, 6));
        assert_eq!(2, byte_to_line_idx(text, 9));
        assert_eq!(3, byte_to_line_idx(text, 10));
        assert_eq!(3, byte_to_line_idx(text, 14));
        assert_eq!(4, byte_to_line_idx(text, 15));
        assert_eq!(4, byte_to_line_idx(text, 20));
        assert_eq!(5, byte_to_line_idx(text, 21));
    }

    #[test]
    fn byte_to_line_idx_03() {
        let text = "Here\r\nare\r\nsome\r\nwords";
        assert_eq!(0, byte_to_line_idx(text, 0));
        assert_eq!(0, byte_to_line_idx(text, 4));
        assert_eq!(0, byte_to_line_idx(text, 5));
        assert_eq!(1, byte_to_line_idx(text, 6));
        assert_eq!(1, byte_to_line_idx(text, 9));
        assert_eq!(1, byte_to_line_idx(text, 10));
        assert_eq!(2, byte_to_line_idx(text, 11));
        assert_eq!(2, byte_to_line_idx(text, 15));
        assert_eq!(2, byte_to_line_idx(text, 16));
        assert_eq!(3, byte_to_line_idx(text, 17));
    }

    #[test]
    fn byte_to_line_idx_04() {
        // Line 0
        for i in 0..32 {
            assert_eq!(0, byte_to_line_idx(TEXT_LINES, i));
        }

        // Line 1
        for i in 32..59 {
            assert_eq!(1, byte_to_line_idx(TEXT_LINES, i));
        }

        // Line 2
        for i in 59..88 {
            assert_eq!(2, byte_to_line_idx(TEXT_LINES, i));
        }

        // Line 3
        for i in 88..125 {
            assert_eq!(3, byte_to_line_idx(TEXT_LINES, i));
        }

        // Past the end
        for i in 125..130 {
            assert_eq!(3, byte_to_line_idx(TEXT_LINES, i));
        }
    }

    #[test]
    fn char_to_byte_idx_01() {
        let text = "Hello せかい!";
        assert_eq!(0, char_to_byte_idx(text, 0));
        assert_eq!(1, char_to_byte_idx(text, 1));
        assert_eq!(2, char_to_byte_idx(text, 2));
        assert_eq!(5, char_to_byte_idx(text, 5));
        assert_eq!(6, char_to_byte_idx(text, 6));
        assert_eq!(12, char_to_byte_idx(text, 8));
        assert_eq!(15, char_to_byte_idx(text, 9));
        assert_eq!(16, char_to_byte_idx(text, 10));
    }

    #[test]
    fn char_to_byte_idx_02() {
        let text = "せかい";
        assert_eq!(0, char_to_byte_idx(text, 0));
        assert_eq!(3, char_to_byte_idx(text, 1));
        assert_eq!(6, char_to_byte_idx(text, 2));
        assert_eq!(9, char_to_byte_idx(text, 3));
    }

    #[test]
    fn char_to_byte_idx_03() {
        let text = "Hello world!";
        assert_eq!(0, char_to_byte_idx(text, 0));
        assert_eq!(1, char_to_byte_idx(text, 1));
        assert_eq!(8, char_to_byte_idx(text, 8));
        assert_eq!(11, char_to_byte_idx(text, 11));
        assert_eq!(12, char_to_byte_idx(text, 12));
    }

    #[test]
    fn char_to_byte_idx_04() {
        let text = "Hello world! Hello せかい! Hello world! Hello せかい! \
                    Hello world! Hello せかい! Hello world! Hello せかい! \
                    Hello world! Hello せかい! Hello world! Hello せかい! \
                    Hello world! Hello せかい! Hello world! Hello せかい!";
        assert_eq!(0, char_to_byte_idx(text, 0));
        assert_eq!(30, char_to_byte_idx(text, 24));
        assert_eq!(60, char_to_byte_idx(text, 48));
        assert_eq!(90, char_to_byte_idx(text, 72));
        assert_eq!(115, char_to_byte_idx(text, 93));
        assert_eq!(120, char_to_byte_idx(text, 96));
        assert_eq!(150, char_to_byte_idx(text, 120));
        assert_eq!(180, char_to_byte_idx(text, 144));
        assert_eq!(210, char_to_byte_idx(text, 168));
        assert_eq!(239, char_to_byte_idx(text, 191));
    }

    #[test]
    fn char_to_byte_idx_05() {
        // Ascii range
        for i in 0..88 {
            assert_eq!(i, char_to_byte_idx(TEXT_LINES, i));
        }

        // Hiragana characters
        for i in 88..100 {
            assert_eq!(88 + ((i - 88) * 3), char_to_byte_idx(TEXT_LINES, i));
        }

        // Past the end
        for i in 100..110 {
            assert_eq!(124, char_to_byte_idx(TEXT_LINES, i));
        }
    }

    #[test]
    fn char_to_line_idx_01() {
        let text = "Hello せ\nか\nい!";
        assert_eq!(0, char_to_line_idx(text, 0));
        assert_eq!(0, char_to_line_idx(text, 7));
        assert_eq!(1, char_to_line_idx(text, 8));
        assert_eq!(1, char_to_line_idx(text, 9));
        assert_eq!(2, char_to_line_idx(text, 10));
    }

    #[test]
    fn char_to_line_idx_02() {
        // Line 0
        for i in 0..32 {
            assert_eq!(0, char_to_line_idx(TEXT_LINES, i));
        }

        // Line 1
        for i in 32..59 {
            assert_eq!(1, char_to_line_idx(TEXT_LINES, i));
        }

        // Line 2
        for i in 59..88 {
            assert_eq!(2, char_to_line_idx(TEXT_LINES, i));
        }

        // Line 3
        for i in 88..100 {
            assert_eq!(3, char_to_line_idx(TEXT_LINES, i));
        }

        // Past the end
        for i in 100..110 {
            assert_eq!(3, char_to_line_idx(TEXT_LINES, i));
        }
    }

    #[test]
    fn line_to_byte_idx_01() {
        let text = "Here\r\nare\r\nsome\r\nwords";
        assert_eq!(0, line_to_byte_idx(text, 0));
        assert_eq!(6, line_to_byte_idx(text, 1));
        assert_eq!(11, line_to_byte_idx(text, 2));
        assert_eq!(17, line_to_byte_idx(text, 3));
    }

    #[test]
    fn line_to_byte_idx_02() {
        let text = "\nHere\nare\nsome\nwords\n";
        assert_eq!(0, line_to_byte_idx(text, 0));
        assert_eq!(1, line_to_byte_idx(text, 1));
        assert_eq!(6, line_to_byte_idx(text, 2));
        assert_eq!(10, line_to_byte_idx(text, 3));
        assert_eq!(15, line_to_byte_idx(text, 4));
        assert_eq!(21, line_to_byte_idx(text, 5));
    }

    #[test]
    fn line_to_byte_idx_03() {
        assert_eq!(0, line_to_byte_idx(TEXT_LINES, 0));
        assert_eq!(32, line_to_byte_idx(TEXT_LINES, 1));
        assert_eq!(59, line_to_byte_idx(TEXT_LINES, 2));
        assert_eq!(88, line_to_byte_idx(TEXT_LINES, 3));

        // Past end
        assert_eq!(124, line_to_byte_idx(TEXT_LINES, 4));
        assert_eq!(124, line_to_byte_idx(TEXT_LINES, 5));
        assert_eq!(124, line_to_byte_idx(TEXT_LINES, 6));
    }

    #[test]
    fn line_to_char_idx_01() {
        let text = "Hello せ\nか\nい!";
        assert_eq!(0, line_to_char_idx(text, 0));
        assert_eq!(8, line_to_char_idx(text, 1));
        assert_eq!(10, line_to_char_idx(text, 2));
    }

    #[test]
    fn line_to_char_idx_02() {
        assert_eq!(0, line_to_char_idx(TEXT_LINES, 0));
        assert_eq!(32, line_to_char_idx(TEXT_LINES, 1));
        assert_eq!(59, line_to_char_idx(TEXT_LINES, 2));
        assert_eq!(88, line_to_char_idx(TEXT_LINES, 3));

        // Past end
        assert_eq!(100, line_to_char_idx(TEXT_LINES, 4));
        assert_eq!(100, line_to_char_idx(TEXT_LINES, 5));
        assert_eq!(100, line_to_char_idx(TEXT_LINES, 6));
    }

    #[test]
    fn line_byte_round_trip() {
        let text = "\nHere\nare\nsome\nwords\n";
        assert_eq!(6, line_to_byte_idx(text, byte_to_line_idx(text, 6)));
        assert_eq!(2, byte_to_line_idx(text, line_to_byte_idx(text, 2)));

        assert_eq!(0, line_to_byte_idx(text, byte_to_line_idx(text, 0)));
        assert_eq!(0, byte_to_line_idx(text, line_to_byte_idx(text, 0)));

        assert_eq!(21, line_to_byte_idx(text, byte_to_line_idx(text, 21)));
        assert_eq!(5, byte_to_line_idx(text, line_to_byte_idx(text, 5)));
    }

    #[test]
    fn line_char_round_trip() {
        let text = "\nHere\nare\nsome\nwords\n";
        assert_eq!(6, line_to_char_idx(text, char_to_line_idx(text, 6)));
        assert_eq!(2, char_to_line_idx(text, line_to_char_idx(text, 2)));

        assert_eq!(0, line_to_char_idx(text, char_to_line_idx(text, 0)));
        assert_eq!(0, char_to_line_idx(text, line_to_char_idx(text, 0)));

        assert_eq!(21, line_to_char_idx(text, char_to_line_idx(text, 21)));
        assert_eq!(5, char_to_line_idx(text, line_to_char_idx(text, 5)));
    }

    #[test]
    fn has_bytes_less_than_01() {
        let v: usize = 0x0709080905090609;
        assert!(v.has_bytes_less_than(0x0A));
        assert!(v.has_bytes_less_than(0x06));
        assert!(!v.has_bytes_less_than(0x05));
    }

    #[test]
    fn flag_bytes_01() {
        let v: usize = 0xE2_09_08_A6_E2_A6_E2_09;
        assert_eq!(0x00_00_00_00_00_00_00_00, v.cmp_eq_byte(0x07));
        assert_eq!(0x00_00_01_00_00_00_00_00, v.cmp_eq_byte(0x08));
        assert_eq!(0x00_01_00_00_00_00_00_01, v.cmp_eq_byte(0x09));
        assert_eq!(0x00_00_00_01_00_01_00_00, v.cmp_eq_byte(0xA6));
        assert_eq!(0x01_00_00_00_01_00_01_00, v.cmp_eq_byte(0xE2));
    }
}
