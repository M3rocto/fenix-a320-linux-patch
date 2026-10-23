/* SPDX-License-Identifier: MIT */
/* Original regression: use a fresh prefix with distinct C and D drive roots.
   Discover their live NT identities; never infer a drive from a volume number. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winternl.h>
#include <stdio.h>
#include <wchar.h>

static int failures;
typedef NTSTATUS (NTAPI *nt_open_file_fn)(PHANDLE,ACCESS_MASK,POBJECT_ATTRIBUTES,PIO_STATUS_BLOCK,ULONG,ULONG);

static void inspect(WCHAR drive)
{
    WCHAR file_path[]=L"C:\\volume-roundtrip.txt";
    WCHAR root[]=L"C:\\", device[1024], final[2048], global[4096], volume_root[2048];
    WCHAR drive_name[]=L"C:";
    HANDLE file, reopened;
    DWORD length, written, error;
    BY_HANDLE_FILE_INFORMATION expected_info, actual_info;
    char expected=(char)drive, actual=0;
    nt_open_file_fn nt_open=(nt_open_file_fn)(void *)GetProcAddress(GetModuleHandleW(L"ntdll.dll"),"NtOpenFile");

    file_path[0]=root[0]=drive_name[0]=drive;
    file=CreateFileW(file_path,GENERIC_READ|GENERIC_WRITE,FILE_SHARE_READ,NULL,CREATE_ALWAYS,0,NULL);
    if(file==INVALID_HANDLE_VALUE){printf("create %lc failed=%lu\n",drive,GetLastError());++failures;return;}
    if(!WriteFile(file,&expected,1,&written,NULL) || written!=1)++failures;
    length=QueryDosDeviceW(drive_name,device,ARRAYSIZE(device));
    printf("%lc QueryDosDevice=%ls length=%lu\n",drive,length?device:L"FAIL",length);
    if(!length){++failures;CloseHandle(file);return;}
    length=GetFinalPathNameByHandleW(file,final,ARRAYSIZE(final),VOLUME_NAME_NT);
    printf("%lc final NT=%ls length=%lu\n",drive,length?final:L"FAIL",length);
    if(!length || length>=ARRAYSIZE(final)){++failures;CloseHandle(file);return;}
    if(nt_open)
    {
        UNICODE_STRING name;
        OBJECT_ATTRIBUTES attr={0};
        IO_STATUS_BLOCK io;
        NTSTATUS status;
        name.Buffer=final;name.Length=(USHORT)(length*sizeof(WCHAR));name.MaximumLength=name.Length+sizeof(WCHAR);
        attr.Length=sizeof(attr);attr.ObjectName=&name;attr.Attributes=OBJ_CASE_INSENSITIVE;
        status=nt_open(&reopened,FILE_READ_ATTRIBUTES,&attr,&io,FILE_SHARE_READ|FILE_SHARE_WRITE,FILE_OPEN_REPARSE_POINT);
        printf("%lc direct NT path open status=%#lx\n",drive,(ULONG)status);
        if(!status)CloseHandle(reopened);else ++failures;
    }
    else ++failures;
    swprintf(global,ARRAYSIZE(global),L"\\\\?\\GLOBALROOT%ls",final);
    SetLastError(0xdeadbeef);
    reopened=CreateFileW(global,GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE,NULL,OPEN_EXISTING,FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT,NULL);
    error=GetLastError();
    if(reopened!=INVALID_HANDLE_VALUE)
    {
        if(!ReadFile(reopened,&actual,1,&written,NULL) || written!=1 || actual!=expected)++failures;
        CloseHandle(reopened);
    }
    else ++failures;
    printf("%lc GLOBALROOT file reopen=%d error=%lu identity=%c\n",drive,reopened!=INVALID_HANDLE_VALUE,error,actual?actual:'?');
    swprintf(volume_root,ARRAYSIZE(volume_root),L"\\\\?\\GLOBALROOT%ls\\",device);
    SetLastError(0xdeadbeef);
    reopened=CreateFileW(volume_root,FILE_READ_ATTRIBUTES,FILE_SHARE_READ,NULL,OPEN_EXISTING,FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT,NULL);
    error=GetLastError();
    printf("%lc GLOBALROOT root=%ls open=%d error=%lu\n",drive,volume_root,reopened!=INVALID_HANDLE_VALUE,error);
    if(reopened!=INVALID_HANDLE_VALUE)
    {
        HANDLE expected_root=CreateFileW(root,FILE_READ_ATTRIBUTES,FILE_SHARE_READ,NULL,OPEN_EXISTING,FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT,NULL);
        int identical=expected_root!=INVALID_HANDLE_VALUE &&
            GetFileInformationByHandle(expected_root,&expected_info) && GetFileInformationByHandle(reopened,&actual_info) &&
            expected_info.dwVolumeSerialNumber==actual_info.dwVolumeSerialNumber &&
            expected_info.nFileIndexHigh==actual_info.nFileIndexHigh && expected_info.nFileIndexLow==actual_info.nFileIndexLow;
        printf("%lc volume root file-identity=%d\n",drive,identical);
        if(!identical)++failures;
        if(expected_root!=INVALID_HANDLE_VALUE)CloseHandle(expected_root);
        CloseHandle(reopened);
    }
    else ++failures;
    CloseHandle(file);
}

int main(void)
{
    const WCHAR *invalid[]={L"\\\\?\\GLOBALROOT\\Device\\HarddiskVolume\\",L"\\\\?\\GLOBALROOT\\Device\\HarddiskVolume65535\\",L"\\\\?\\GLOBALROOTX\\Device\\HarddiskVolume1\\"};
    WCHAR c_device[1024],d_device[1024];
    unsigned int i;
    setvbuf(stdout,NULL,_IONBF,0);
    if(!QueryDosDeviceW(L"C:",c_device,ARRAYSIZE(c_device)) ||
       !QueryDosDeviceW(L"D:",d_device,ARRAYSIZE(d_device)) || !wcscmp(c_device,d_device))
    {
        puts("The disposable prefix must have distinct C and D NT-device identities");
        return 1;
    }
    inspect(L'C');
    inspect(L'D');
    for(i=0;i<ARRAYSIZE(invalid);++i)
    {
        HANDLE file=CreateFileW(invalid[i],FILE_READ_ATTRIBUTES,FILE_SHARE_READ,NULL,OPEN_EXISTING,FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT,NULL);
        printf("invalid device path %u rejected=%d error=%lu\n",i,file==INVALID_HANDLE_VALUE,GetLastError());
        if(file!=INVALID_HANDLE_VALUE){CloseHandle(file);++failures;}
    }
    printf("volume round-trip failures=%d\n",failures);
    return failures?1:0;
}
