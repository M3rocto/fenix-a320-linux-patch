/*
 * wintrust softpub functions tests
 *
 * Copyright 2007,2010 Juan Lang
 * Copyright 2010 Andrey Turkin
 * Copyright 2016 Mark Jansen
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301, USA
 */

#include <windows.h>
#include <stdio.h>
/* minimal PE file image */
#define VA_START 0x400000
#define FILE_PE_START 0x50
#define NUM_SECTIONS 3
#define FILE_TEXT 0x200
#define RVA_TEXT 0x1000
#define RVA_BSS 0x2000
#define FILE_IDATA 0x400
#define RVA_IDATA 0x3000
#define FILE_TOTAL 0x600
#define RVA_TOTAL 0x4000
#pragma pack(push,1)
struct Imports {
    IMAGE_IMPORT_DESCRIPTOR descriptors[2];
    IMAGE_THUNK_DATA32 original_thunks[2];
    IMAGE_THUNK_DATA32 thunks[2];
    struct __IMPORT_BY_NAME {
        WORD hint;
        char funcname[0x20];
    } ibn;
    char dllname[0x10];
};
#define EXIT_PROCESS (VA_START+RVA_IDATA+FIELD_OFFSET(struct Imports, thunks))

static struct _PeImage {
    IMAGE_DOS_HEADER dos_header;
    char __alignment1[FILE_PE_START - sizeof(IMAGE_DOS_HEADER)];
    IMAGE_NT_HEADERS32 nt_headers;
    IMAGE_SECTION_HEADER sections[NUM_SECTIONS];
    char __alignment2[FILE_TEXT - FILE_PE_START - sizeof(IMAGE_NT_HEADERS32) -
        NUM_SECTIONS * sizeof(IMAGE_SECTION_HEADER)];
    unsigned char text_section[FILE_IDATA-FILE_TEXT];
    struct Imports idata_section;
    char __alignment3[FILE_TOTAL-FILE_IDATA-sizeof(struct Imports)];
} bin = {
    /* dos header */
    {IMAGE_DOS_SIGNATURE, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, {0}, 0, 0, {0}, FILE_PE_START},
    /* alignment before PE header */
    {0},
    /* nt headers */
    {IMAGE_NT_SIGNATURE,
        /* basic headers - 3 sections, no symbols, EXE file */
        {IMAGE_FILE_MACHINE_I386, NUM_SECTIONS, 0, 0, 0, sizeof(IMAGE_OPTIONAL_HEADER32),
            IMAGE_FILE_32BIT_MACHINE | IMAGE_FILE_EXECUTABLE_IMAGE},
        /* optional header */
        {IMAGE_NT_OPTIONAL_HDR32_MAGIC, 4, 0, FILE_IDATA-FILE_TEXT,
            FILE_TOTAL-FILE_IDATA + FILE_IDATA-FILE_TEXT, 0x400,
            RVA_TEXT, RVA_TEXT, RVA_BSS, VA_START, 0x1000, 0x200, 4, 0, 1, 0, 4, 0, 0,
            RVA_TOTAL, FILE_TEXT, 0, IMAGE_SUBSYSTEM_WINDOWS_GUI, 0,
            0x200000, 0x1000, 0x100000, 0x1000, 0, 0x10,
            {{0, 0},
             {RVA_IDATA, sizeof(struct Imports)}
            }
        }
    },
    /* sections */
    {
        {".text", {0x100}, RVA_TEXT, FILE_IDATA-FILE_TEXT, FILE_TEXT,
            0, 0, 0, 0, IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ},
        {".bss", {0x400}, RVA_BSS, 0, 0, 0, 0, 0, 0,
            IMAGE_SCN_CNT_UNINITIALIZED_DATA | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE},
        {".idata", {sizeof(struct Imports)}, RVA_IDATA, FILE_TOTAL-FILE_IDATA, FILE_IDATA, 0,
            0, 0, 0, IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE}
    },
    /* alignment before first section */
    {0},
    /* .text section */
    {
        0x31, 0xC0, /* xor eax, eax */
        0xFF, 0x25, EXIT_PROCESS&0xFF, (EXIT_PROCESS>>8)&0xFF, (EXIT_PROCESS>>16)&0xFF,
            (EXIT_PROCESS>>24)&0xFF, /* jmp ExitProcess */
        0
    },
    /* .idata section */
    {
        {
            {{RVA_IDATA + FIELD_OFFSET(struct Imports, original_thunks)}, 0, 0,
            RVA_IDATA + FIELD_OFFSET(struct Imports, dllname),
            RVA_IDATA + FIELD_OFFSET(struct Imports, thunks)
            },
            {{0}, 0, 0, 0, 0}
        },
        {{{RVA_IDATA+FIELD_OFFSET(struct Imports, ibn)}}, {{0}}},
        {{{RVA_IDATA+FIELD_OFFSET(struct Imports, ibn)}}, {{0}}},
        {0,"ExitProcess"},
        "KERNEL32.DLL"
    },
    /* final alignment */
    {0}
};
#pragma pack(pop)
