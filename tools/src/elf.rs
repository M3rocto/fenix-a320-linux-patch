// SPDX-License-Identifier: MIT
//! Just enough of the ELF format to tell whether a program runs on any
//! x86-64 Linux, whatever C library and loader that system has.
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not an ELF program for x86-64 at all, or one that cannot be read.
    NotExecutable,
    /// Needs a program interpreter or shared libraries of the system it runs on.
    Dynamic,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotExecutable => "not an x86-64 ELF executable",
            Self::Dynamic => "not statically linked",
        })
    }
}

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
/// The sizes of the ELF header, a program header and a dynamic entry in a 64-bit file.
const HEADER: usize = 64;
const PROGRAM_HEADER: usize = 56;
const DYNAMIC_ENTRY: usize = 16;

fn bytes<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N], Refusal> {
    offset
        .checked_add(N)
        .and_then(|end| data.get(offset..end))
        .and_then(|found| found.try_into().ok())
        .ok_or(Refusal::NotExecutable)
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, Refusal> {
    bytes(data, offset).map(u16::from_le_bytes)
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, Refusal> {
    bytes(data, offset).map(u32::from_le_bytes)
}

fn u64_at(data: &[u8], offset: usize) -> Result<u64, Refusal> {
    bytes(data, offset).map(u64::from_le_bytes)
}

fn offset_at(data: &[u8], offset: usize) -> Result<usize, Refusal> {
    usize::try_from(u64_at(data, offset)?).map_err(|_| Refusal::NotExecutable)
}

/// Accept a little-endian ELF64 program for x86-64 that names no program
/// interpreter and needs no shared library. A static position-independent
/// executable has a dynamic section for its own relocations; that is fine.
pub fn check_static(data: &[u8]) -> Result<(), Refusal> {
    if data.len() < HEADER || bytes(data, 0)? != *b"\x7fELF" {
        return Err(Refusal::NotExecutable);
    }
    let (class, encoding) = (data[4], data[5]);
    let kind = u16_at(data, 16)?;
    if class != 2
        || encoding != 1
        || !matches!(kind, ET_EXEC | ET_DYN)
        || u16_at(data, 18)? != EM_X86_64
    {
        return Err(Refusal::NotExecutable);
    }
    let table = offset_at(data, 32)?;
    let (size, count) = (u16_at(data, 54)?, u16_at(data, 56)?);
    // A program without segments cannot run, and 0xffff stands for a count kept elsewhere.
    if usize::from(size) != PROGRAM_HEADER || count == 0 || count == u16::MAX {
        return Err(Refusal::NotExecutable);
    }
    let mut dynamic = Vec::new();
    for index in 0..usize::from(count) {
        let header = index
            .checked_mul(PROGRAM_HEADER)
            .and_then(|offset| offset.checked_add(table))
            .ok_or(Refusal::NotExecutable)?;
        match u32_at(data, header)? {
            PT_INTERP => return Err(Refusal::Dynamic),
            PT_DYNAMIC => {
                dynamic.push((offset_at(data, header + 8)?, offset_at(data, header + 32)?))
            }
            _ => {
                // Still read to its end, so that a cut-off table is noticed.
                bytes::<PROGRAM_HEADER>(data, header)?;
            }
        }
    }
    for (start, length) in dynamic {
        let section = start
            .checked_add(length)
            .and_then(|end| data.get(start..end))
            .ok_or(Refusal::NotExecutable)?;
        for entry in section.as_chunks::<DYNAMIC_ENTRY>().0 {
            match u64_at(entry, 0)? {
                DT_NULL => break,
                DT_NEEDED => return Err(Refusal::Dynamic),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A program header with the given type and file range.
    fn segment(kind: u32, offset: u64, size: u64) -> [u8; PROGRAM_HEADER] {
        let mut header = [0u8; PROGRAM_HEADER];
        header[..4].copy_from_slice(&kind.to_le_bytes());
        header[8..16].copy_from_slice(&offset.to_le_bytes());
        header[32..40].copy_from_slice(&size.to_le_bytes());
        header
    }

    /// An ELF64 file with the given program headers, followed by `tail`.
    fn elf(kind: u16, machine: u16, segments: &[[u8; PROGRAM_HEADER]], tail: &[u8]) -> Vec<u8> {
        let mut file = vec![0u8; HEADER];
        file[..6].copy_from_slice(b"\x7fELF\x02\x01");
        file[6] = 1;
        file[16..18].copy_from_slice(&kind.to_le_bytes());
        file[18..20].copy_from_slice(&machine.to_le_bytes());
        file[32..40].copy_from_slice(&(HEADER as u64).to_le_bytes());
        file[54..56].copy_from_slice(&(PROGRAM_HEADER as u16).to_le_bytes());
        file[56..58].copy_from_slice(&(segments.len() as u16).to_le_bytes());
        for segment in segments {
            file.extend_from_slice(segment);
        }
        file.extend_from_slice(tail);
        file
    }

    fn entries(tags: &[u64]) -> Vec<u8> {
        tags.iter()
            .flat_map(|tag| [tag.to_le_bytes(), 7u64.to_le_bytes()].concat())
            .collect()
    }

    const LOAD: u32 = 1;

    #[test]
    fn static_programs_pass_with_or_without_a_dynamic_section() {
        let load = segment(LOAD, 0, 64);
        assert_eq!(check_static(&elf(ET_EXEC, EM_X86_64, &[load], b"")), Ok(()));
        // Static PIE: DT_FLAGS_1, DT_RELA and the like, but nothing is needed.
        let at = (HEADER + 2 * PROGRAM_HEADER) as u64;
        let own = entries(&[0x6fff_fffb, 7, 8, 9, 0x6fff_fff9, DT_NULL, DT_NEEDED]);
        let pie = elf(
            ET_DYN,
            EM_X86_64,
            &[load, segment(PT_DYNAMIC, at, own.len() as u64)],
            &own,
        );
        assert_eq!(check_static(&pie), Ok(()));
    }

    #[test]
    fn interpreters_and_needed_libraries_are_refused() {
        let load = segment(LOAD, 0, 64);
        let interp = elf(ET_DYN, EM_X86_64, &[load, segment(PT_INTERP, 0, 0)], b"");
        assert_eq!(check_static(&interp), Err(Refusal::Dynamic));
        let at = (HEADER + 2 * PROGRAM_HEADER) as u64;
        let needs = entries(&[0x1d, DT_NEEDED, DT_NULL]);
        let shared = elf(
            ET_DYN,
            EM_X86_64,
            &[load, segment(PT_DYNAMIC, at, needs.len() as u64)],
            &needs,
        );
        assert_eq!(check_static(&shared), Err(Refusal::Dynamic));
        assert_eq!(Refusal::Dynamic.to_string(), "not statically linked");
    }

    #[test]
    fn anything_else_is_no_executable() {
        let load = segment(LOAD, 0, 64);
        let good = elf(ET_EXEC, EM_X86_64, &[load], b"");
        let changed = |offset: usize, value: u8| {
            let mut file = good.clone();
            file[offset] = value;
            file
        };
        for (what, file) in [
            ("empty", Vec::new()),
            ("a script", b"#!/bin/sh\nexit 0\n".to_vec()),
            ("magic", changed(0, 0x7e)),
            ("32-bit", changed(4, 1)),
            ("big-endian", changed(5, 2)),
            ("relocatable object", changed(16, 1)),
            ("core dump", changed(16, 4)),
            ("aarch64", elf(ET_EXEC, 183, &[load], b"")),
            ("i386", elf(ET_EXEC, 3, &[load], b"")),
            ("header size", changed(54, 32)),
            ("no segments", elf(ET_EXEC, EM_X86_64, &[], b"")),
            ("cut off in the header", good[..40].to_vec()),
            ("cut off in the table", good[..HEADER + 20].to_vec()),
            ("table beyond the file", changed(33, 0x10)),
            (
                "dynamic section beyond the file",
                elf(
                    ET_DYN,
                    EM_X86_64,
                    &[load, segment(PT_DYNAMIC, 4096, 32)],
                    b"",
                ),
            ),
            (
                "dynamic section wraps around",
                elf(
                    ET_DYN,
                    EM_X86_64,
                    &[load, segment(PT_DYNAMIC, u64::MAX - 4, 32)],
                    b"",
                ),
            ),
        ] {
            assert_eq!(check_static(&file), Err(Refusal::NotExecutable), "{what}");
        }
        assert_eq!(
            Refusal::NotExecutable.to_string(),
            "not an x86-64 ELF executable"
        );
    }

    #[test]
    #[cfg(target_env = "gnu")]
    fn the_running_test_program_is_dynamic() {
        // Built for glibc, it needs the system's loader and libc.so.6.
        let own = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(check_static(&own), Err(Refusal::Dynamic));
    }
}
