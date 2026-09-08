// Diagnostic only: request HID rediscovery in an already running CrossOver Steam.
// No DLL injection, process restart, registry changes, or persistent installation.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <dbt.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static DWORD steam_pid;
static int send_arrival;
static unsigned int detected;
static unsigned int delivered;

static DWORD find_steam(void)
{
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    PROCESSENTRY32 entry = {0};
    DWORD found = 0;
    entry.dwSize = sizeof(entry);
    if (snapshot == INVALID_HANDLE_VALUE) return 0;
    if (Process32First(snapshot, &entry)) {
        do {
            if (_stricmp(entry.szExeFile, "steam.exe") == 0) {
                if (found) {
                    fprintf(stderr, "Multiple steam.exe processes; refusing ambiguous target\n");
                    CloseHandle(snapshot);
                    return 0;
                }
                found = entry.th32ProcessID;
            }
        } while (Process32Next(snapshot, &entry));
    }
    CloseHandle(snapshot);
    return found;
}

static void list_modules(void)
{
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, steam_pid);
    MODULEENTRY32 entry = {0};
    entry.dwSize = sizeof(entry);
    if (snapshot == INVALID_HANDLE_VALUE) return;
    if (Module32First(snapshot, &entry)) {
        do {
            if (_stricmp(entry.szModule, "hid.dll") == 0 ||
                _stricmp(entry.szModule, "SDL3.dll") == 0) {
                printf("module=%s path=%s\n", entry.szModule, entry.szExePath);
            }
        } while (Module32Next(snapshot, &entry));
    }
    CloseHandle(snapshot);
}

static BOOL CALLBACK visit_window(HWND window, LPARAM unused)
{
    DWORD pid = 0;
    char class_name[256] = {0};
    (void)unused;
    GetWindowThreadProcessId(window, &pid);
    if (pid != steam_pid) return TRUE;
    GetClassNameA(window, class_name, sizeof(class_name));
    printf("window=%p pid=%lu class=%s\n", (void *)window, (unsigned long)pid, class_name);
    if (strcmp(class_name, "SDL_HIDAPI_DEVICE_DETECTION") != 0) return TRUE;
    ++detected;
    if (send_arrival) {
        // WM_DEVICECHANGE is a system message; SendMessageTimeout marshals the
        // sized interface payload across processes. Do not post a raw pointer.
        DEV_BROADCAST_DEVICEINTERFACE_A event = {0};
        DWORD_PTR result = 0;
        event.dbcc_size = sizeof(event);
        event.dbcc_devicetype = DBT_DEVTYP_DEVICEINTERFACE;
        event.dbcc_classguid = (GUID){0x4d1e55b2, 0xf16f, 0x11cf,
            {0x88, 0xcb, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30}};
        SetLastError(0);
        LRESULT sent = SendMessageTimeoutA(window, WM_DEVICECHANGE, DBT_DEVICEARRIVAL,
            (LPARAM)&event, SMTO_ABORTIFHUNG | SMTO_BLOCK, 2000, &result);
        printf("arrival window=%p sent=%lld result=%llu error=%lu\n", (void *)window,
            (long long)sent, (unsigned long long)result, (unsigned long)GetLastError());
        if (sent) ++delivered;
    }
    return TRUE;
}

int main(int argc, char **argv)
{
    if (argc != 2 || (strcmp(argv[1], "--list") != 0 && strcmp(argv[1], "--arrival") != 0)) {
        fprintf(stderr, "Usage: notify.exe --list|--arrival\n");
        return 2;
    }
    send_arrival = strcmp(argv[1], "--arrival") == 0;
    steam_pid = find_steam();
    if (!steam_pid) {
        fprintf(stderr, "Exactly one running steam.exe is required\n");
        return 3;
    }
    printf("steam_pid=%lu mode=%s tick_ms=%llu\n", (unsigned long)steam_pid, argv[1],
        (unsigned long long)GetTickCount64());
    list_modules();
    EnumWindows(visit_window, 0);
    // SDL creates a message-only window; EnumWindows alone cannot find it.
    HWND window = NULL;
    while ((window = FindWindowExA(HWND_MESSAGE, window, NULL, NULL)) != NULL) {
        visit_window(window, 0);
    }
    printf("detection_windows=%u notifications_delivered=%u\n", detected, delivered);
    fflush(stdout);
    return send_arrival ? (delivered ? 0 : 4) : (detected ? 0 : 5);
}
