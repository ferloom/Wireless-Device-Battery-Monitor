#pragma once

#include <windows.h>

constexpr DWORD kHostTestMagic = 0x48424858; // HBHX
constexpr DWORD kHostTestVersion = 2;
constexpr DWORD kHostTestWatchdogMs = 60000;
constexpr DWORD kExperimentalWatchdogMs = 24 * 60 * 60 * 1000;
constexpr wchar_t kHostTestMessageName[] = L"BatteryHub.TaskbarXamlHostTest.2";
constexpr wchar_t kHostTestMappingPrefix[] = L"Local\\BatteryHub.TaskbarXamlHostTest.2.";
constexpr wchar_t kExperimentalStopEvent[] = L"Local\\BatteryHub.Experimental.TaskbarXaml.Stop";

enum HostTestState : LONG {
    HostIdle = 0,
    HostAttachRequested = 1,
    HostAttachRunning = 2,
    HostAttached = 3,
    HostDetachRequested = 4,
    HostDetachRunning = 5,
    HostFailed = 6,
    HostDetached = 7,
};

struct HostTestSharedState {
    DWORD magic;
    DWORD version;
    DWORD targetPid;
    DWORD targetThreadId;
    HWND taskbarWindow;
    UINT_PTR timerId;
    DWORD watchdogMs;
    volatile LONG state;
    HRESULT attachResult;
    HRESULT detachResult;
    wchar_t logPath[512];
};

using HostTestHookProc = LRESULT (CALLBACK*)(int, WPARAM, LPARAM);
