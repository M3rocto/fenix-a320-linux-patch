/* SPDX-License-Identifier: MIT */
/* Original regression: compare native-device aliases with the existing DOS
   controls. Run only in a disposable prefix with distinct C and D roots. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winternl.h>
#include <stdio.h>
#include <wchar.h>

typedef NTSTATUS (NTAPI *nt_open_file_fn)(PHANDLE,ACCESS_MASK,POBJECT_ATTRIBUTES,PIO_STATUS_BLOCK,ULONG,ULONG);
static unsigned int failures;

static void inspect_systemroot(void)
{
    WCHAR windows[MAX_PATH], filename[MAX_PATH], path[MAX_PATH], separators[4];
    const WCHAR basename[]=L"fenix-own-systemroot-probe.txt";
    nt_open_file_fn nt_open=(nt_open_file_fn)(void *)GetProcAddress(GetModuleHandleW(L"ntdll.dll"),"NtOpenFile");
    DWORD length, bytes;
    HANDLE file;
    unsigned int count, i;
    char expected='S';
    length=GetWindowsDirectoryW(windows,ARRAYSIZE(windows));
    if(!nt_open || !length || length+ARRAYSIZE(basename)+1>ARRAYSIZE(filename)){++failures;return;}
    swprintf(filename,ARRAYSIZE(filename),L"%ls\\%ls",windows,basename);
    file=CreateFileW(filename,GENERIC_WRITE,0,NULL,CREATE_ALWAYS,0,NULL);
    if(file==INVALID_HANDLE_VALUE){++failures;return;}
    if(!WriteFile(file,&expected,1,&bytes,NULL) || bytes!=1)++failures;
    CloseHandle(file);
    for(count=1;count<=3;++count)
    {
        UNICODE_STRING name;
        OBJECT_ATTRIBUTES attr={0};
        IO_STATUS_BLOCK io;
        NTSTATUS status;
        for(i=0;i<count;++i)separators[i]='\\';
        separators[count]=0;
        swprintf(path,ARRAYSIZE(path),L"\\SystemRoot%ls%ls",separators,basename);
        name.Buffer=path;name.Length=(USHORT)(wcslen(path)*sizeof(WCHAR));name.MaximumLength=name.Length+sizeof(WCHAR);
        attr.Length=sizeof(attr);attr.ObjectName=&name;attr.Attributes=OBJ_CASE_INSENSITIVE;
        status=nt_open(&file,GENERIC_READ,&attr,&io,FILE_SHARE_READ,FILE_SYNCHRONOUS_IO_NONALERT);
        printf("SystemRoot separators=%u status=%#lx\n",count,(ULONG)status);
        if(!status)
        {
            char actual=0;
            if(!ReadFile(file,&actual,1,&bytes,NULL) || bytes!=1 || actual!=expected)++failures;
            CloseHandle(file);
        }
        if((!status)!=(count==1))++failures;
    }
    if(!DeleteFileW(filename))++failures;
}

static void inspect(WCHAR drive)
{
    WCHAR drive_name[]=L"C:", root[]=L"C:\\";
    WCHAR device[1024], path[4096], final[2048], separators[4];
    HANDLE file;
    unsigned int count, kind;
    DWORD length;
    nt_open_file_fn nt_open=(nt_open_file_fn)(void *)GetProcAddress(GetModuleHandleW(L"ntdll.dll"),"NtOpenFile");
    drive_name[0]=root[0]=drive;
    if(!nt_open || !QueryDosDeviceW(drive_name,device,ARRAYSIZE(device))) {++failures;return;}
    file=CreateFileW(root,FILE_READ_ATTRIBUTES,FILE_SHARE_READ,NULL,OPEN_EXISTING,FILE_FLAG_BACKUP_SEMANTICS,NULL);
    if(file==INVALID_HANDLE_VALUE) {++failures;return;}
    length=GetFinalPathNameByHandleW(file,final,ARRAYSIZE(final),VOLUME_NAME_NT);
    printf("%lc root final NT=%ls length=%lu\n",drive,length?final:L"FAIL",length);
    if(!length)++failures;
    CloseHandle(file);
    for(count=1;count<=3;++count)
    {
        for(kind=0;kind<count;++kind)separators[kind]='\\';
        separators[count]=0;
        for(kind=0;kind<4;++kind)
        {
            DWORD error;
            NTSTATUS status=0;
            int success;
            if(kind==0)swprintf(path,ARRAYSIZE(path),L"%lc:%lsvolume-roundtrip.txt",drive,separators);
            else if(kind==1)swprintf(path,ARRAYSIZE(path),L"\\\\?\\%lc:%lsvolume-roundtrip.txt",drive,separators);
            else if(kind==2)swprintf(path,ARRAYSIZE(path),L"%ls%lsvolume-roundtrip.txt",device,separators);
            else swprintf(path,ARRAYSIZE(path),L"\\\\?\\GLOBALROOT%ls%lsvolume-roundtrip.txt",device,separators);
            SetLastError(0xdeadbeef);
            if(kind==2)
            {
                UNICODE_STRING name;
                OBJECT_ATTRIBUTES attr={0};
                IO_STATUS_BLOCK io;
                name.Buffer=path;name.Length=(USHORT)(wcslen(path)*sizeof(WCHAR));name.MaximumLength=name.Length+sizeof(WCHAR);
                attr.Length=sizeof(attr);attr.ObjectName=&name;attr.Attributes=OBJ_CASE_INSENSITIVE;
                status=nt_open(&file,GENERIC_READ,&attr,&io,FILE_SHARE_READ,FILE_SYNCHRONOUS_IO_NONALERT);
                success=!status;error=0;
            }
            else
            {
                file=CreateFileW(path,GENERIC_READ,FILE_SHARE_READ,NULL,OPEN_EXISTING,FILE_FLAG_OPEN_NO_RECALL,NULL);
                error=GetLastError();success=file!=INVALID_HANDLE_VALUE;
            }
            printf("%lc kind=%u separators=%u success=%d error=%lu status=%#lx path=%ls\n",drive,kind,count,success,error,(ULONG)status,path);
            if(success)
            {
                char data=0;DWORD bytes;
                if(!ReadFile(file,&data,1,&bytes,NULL) || bytes!=1 || data!=(char)drive)++failures;
                CloseHandle(file);
            }
            /* Ordinary Win32 paths collapse separators before NT conversion.
               The existing Wine extended-DOS control accepts two at root,
               rejects three. Require native-device aliases to agree. */
            if(success!=(kind==0 || count<=2))++failures;
        }
    }
}

int main(void)
{
    setvbuf(stdout,NULL,_IONBF,0);
    inspect(L'C');inspect(L'D');
    inspect_systemroot();
    printf("volume separator failures=%u\n",failures);
    return failures?1:0;
}
