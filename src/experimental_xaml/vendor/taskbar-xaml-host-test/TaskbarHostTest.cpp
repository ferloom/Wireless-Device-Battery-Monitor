#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shellapi.h>
#include <dbghelp.h>
#include <bcrypt.h>
#include <conio.h>
#include <winternl.h>

#include "HostTestProtocol.h"
#include "../taskbar-xaml-widget/WidgetSurfaceReader.h"

#include <array>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cwchar>
#include <string>
#include <vector>

#pragma comment(lib, "dbghelp.lib")
#pragma comment(lib, "bcrypt.lib")
#pragma comment(lib, "shell32.lib")
#pragma comment(lib, "version.lib")

namespace {
constexpr wchar_t kExpectedTaskbarHash[] = L"A52F99F7BE8CC1EE360025D1C06C1480A489B1D309BEF230C05EB21A106CCF34";
constexpr wchar_t kExpectedPdbHash[] = L"8931A245D372B301976135B6FEDAD2B6C26F3DD4DBA10E9E9BFAD3BEA45C108D";
constexpr GUID kExpectedPdbGuid = {0x8645A6F8, 0xC08F, 0xC8A1, {0x8D, 0xAC, 0xD4, 0xB7, 0x7C, 0xF1, 0xE2, 0xEE}};
constexpr DWORD kExpectedPdbAge = 1;
constexpr DWORD kExpectedBuild = 26300;
constexpr DWORD kExpectedUbr = 9818;

std::atomic<bool> g_cancel{false};

BOOL WINAPI ConsoleControl(DWORD type) {
    if (type == CTRL_C_EVENT || type == CTRL_BREAK_EVENT || type == CTRL_CLOSE_EVENT) {
        g_cancel.store(true);
        return TRUE;
    }
    return FALSE;
}

std::wstring ModuleDirectory() {
    std::wstring path(32768, L'\0');
    DWORD n = GetModuleFileNameW(nullptr, path.data(), static_cast<DWORD>(path.size()));
    if (!n || n >= path.size()) return {};
    path.resize(n);
    const auto slash = path.find_last_of(L"\\/");
    if (slash == std::wstring::npos) return {};
    path.resize(slash);
    return path;
}

bool AppendLog(std::wstring const& path, wchar_t const* event, wchar_t const* detail = L"") {
    SYSTEMTIME now{};
    GetSystemTime(&now);
    wchar_t line[1024]{};
    swprintf_s(line, L"%04u-%02u-%02uT%02u:%02u:%02u.%03uZ pid=%lu event=%s %s\r\n",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds,
        GetCurrentProcessId(), event, detail);
    HANDLE file = CreateFileW(path.c_str(), FILE_APPEND_DATA,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;
    char utf8[sizeof(line) * 4]{};
    const int encodedBytes = WideCharToMultiByte(CP_UTF8, 0, line, -1, utf8, sizeof(utf8), nullptr, nullptr);
    DWORD written = 0;
    const BOOL ok = encodedBytes > 1 && WriteFile(file, utf8, static_cast<DWORD>(encodedBytes - 1), &written, nullptr);
    CloseHandle(file);
    return ok != FALSE;
}

std::wstring Hex(std::array<UCHAR, 32> const& bytes) {
    static wchar_t const digits[] = L"0123456789ABCDEF";
    std::wstring value(bytes.size() * 2, L'0');
    for (size_t i = 0; i < bytes.size(); ++i) {
        value[i * 2] = digits[bytes[i] >> 4];
        value[i * 2 + 1] = digits[bytes[i] & 0xF];
    }
    return value;
}

bool Sha256(std::wstring const& path, std::wstring& result) {
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;
    BCRYPT_ALG_HANDLE alg = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    DWORD objectBytes = 0, resultBytes = 0, got = 0;
    bool ok = BCryptOpenAlgorithmProvider(&alg, BCRYPT_SHA256_ALGORITHM, nullptr, 0) >= 0 &&
        BCryptGetProperty(alg, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectBytes), sizeof(objectBytes), &got, 0) >= 0 &&
        BCryptGetProperty(alg, BCRYPT_HASH_LENGTH, reinterpret_cast<PUCHAR>(&resultBytes), sizeof(resultBytes), &got, 0) >= 0 &&
        resultBytes == 32;
    std::vector<UCHAR> object(objectBytes);
    if (ok) ok = BCryptCreateHash(alg, &hash, object.data(), objectBytes, nullptr, 0, 0) >= 0;
    std::array<UCHAR, 64 * 1024> buffer{};
    while (ok) {
        DWORD read = 0;
        if (!ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &read, nullptr)) { ok = false; break; }
        if (!read) break;
        ok = BCryptHashData(hash, buffer.data(), read, 0) >= 0;
    }
    std::array<UCHAR, 32> digest{};
    if (ok) ok = BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0) >= 0;
    if (hash) BCryptDestroyHash(hash);
    if (alg) BCryptCloseAlgorithmProvider(alg, 0);
    CloseHandle(file);
    if (ok) result = Hex(digest);
    return ok;
}

bool QueryWindowsVersion(DWORD& build, DWORD& ubr, std::wstring& displayVersion) {
    using RtlGetVersionFn = LONG (WINAPI*)(PRTL_OSVERSIONINFOW);
    auto rtlGetVersion = reinterpret_cast<RtlGetVersionFn>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "RtlGetVersion"));
    RTL_OSVERSIONINFOW version{};
    version.dwOSVersionInfoSize = sizeof(version);
    if (!rtlGetVersion || rtlGetVersion(&version) < 0 || version.dwMajorVersion != 10 || version.dwBuildNumber != kExpectedBuild)
        return false;
    build = version.dwBuildNumber;
    HKEY key = nullptr;
    if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, L"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", 0, KEY_QUERY_VALUE, &key) != ERROR_SUCCESS)
        return false;
    DWORD ubrType = 0, bytes = sizeof(ubr);
    const LSTATUS ubrStatus = RegQueryValueExW(key, L"UBR", nullptr, &ubrType, reinterpret_cast<BYTE*>(&ubr), &bytes);
    wchar_t value[64]{};
    DWORD valueBytes = sizeof(value);
    DWORD displayType = 0;
    const LSTATUS displayStatus = RegQueryValueExW(key, L"DisplayVersion", nullptr, &displayType, reinterpret_cast<BYTE*>(value), &valueBytes);
    RegCloseKey(key);
    if (ubrStatus != ERROR_SUCCESS || ubrType != REG_DWORD || bytes != sizeof(ubr) ||
        displayStatus != ERROR_SUCCESS || displayType != REG_SZ) return false;
    displayVersion = value;
    return ubr == kExpectedUbr && displayVersion == L"26H2";
}

bool GetExpectedPaths(std::wstring& taskbarPath, std::wstring& pdbPath, std::wstring& adapterPath) {
    wchar_t windows[MAX_PATH]{};
    UINT n = GetWindowsDirectoryW(windows, ARRAYSIZE(windows));
    const auto dir = ModuleDirectory();
    if (!n || n >= ARRAYSIZE(windows) || dir.empty()) return false;
    taskbarPath = std::wstring(windows) + L"\\System32\\taskbar.dll";
    pdbPath = dir + L"\\Taskbar.pdb";
    adapterPath = dir + L"\\BatteryHubExplorerXamlAdapter.dll";
    return true;
}

bool VerifyPdbWithDbgHelp(std::wstring const& taskbarPath, std::wstring const& pdbPath, std::wstring const& symbolsDir) {
    HANDLE process = GetCurrentProcess();
    SymSetOptions(SYMOPT_DEFERRED_LOADS | SYMOPT_FAIL_CRITICAL_ERRORS);
    if (!SymInitializeW(process, symbolsDir.c_str(), FALSE)) return false;
    const DWORD64 base = SymLoadModuleExW(process, nullptr, taskbarPath.c_str(), L"taskbar.dll", 0, 0, nullptr, 0);
    IMAGEHLP_MODULE64 module{};
    module.SizeOfStruct = sizeof(module);
    const bool loaded = base && SymGetModuleInfo64(process, base, &module) && module.SymType == SymPdb &&
        !module.PdbUnmatched && module.PdbAge == kExpectedPdbAge && IsEqualGUID(module.PdbSig70, kExpectedPdbGuid);
    wchar_t detail[512]{};
    swprintf_s(detail, L"symType=%u unmatched=%u age=%lu guid=%08lX-%04X-%04X-%02X%02X-%02X%02X%02X%02X%02X%02X",
        module.SymType, module.PdbUnmatched, module.PdbAge, module.PdbSig70.Data1, module.PdbSig70.Data2,
        module.PdbSig70.Data3, module.PdbSig70.Data4[0], module.PdbSig70.Data4[1], module.PdbSig70.Data4[2],
        module.PdbSig70.Data4[3], module.PdbSig70.Data4[4], module.PdbSig70.Data4[5], module.PdbSig70.Data4[6],
        module.PdbSig70.Data4[7]);
    SymCleanup(process);
    if (loaded) wprintf(L"PDB matched: %hs; %s\n", module.LoadedPdbName, detail);
    else wprintf(L"PDB verification failed: %s\n", detail);
    return loaded;
}

bool VerifyExplorer(HWND& taskbar, DWORD& explorerPid, DWORD& explorerTid, HANDLE& explorerProcess) {
    taskbar = FindWindowW(L"Shell_TrayWnd", nullptr);
    if (!taskbar) return false;
    explorerTid = GetWindowThreadProcessId(taskbar, &explorerPid);
    if (!explorerTid || !explorerPid) return false;
    explorerProcess = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, explorerPid);
    if (!explorerProcess) return false;
    wchar_t image[MAX_PATH]{};
    DWORD imageChars = ARRAYSIZE(image);
    if (!QueryFullProcessImageNameW(explorerProcess, 0, image, &imageChars)) return false;
    wchar_t expected[MAX_PATH]{};
    DWORD windowsChars = GetWindowsDirectoryW(expected, ARRAYSIZE(expected));
    if (!windowsChars || windowsChars >= ARRAYSIZE(expected)) return false;
    wcscat_s(expected, L"\\explorer.exe");
    if (_wcsicmp(image, expected) != 0) return false;
    USHORT processMachine = 0, nativeMachine = 0;
    if (!IsWow64Process2(explorerProcess, &processMachine, &nativeMachine) || processMachine != IMAGE_FILE_MACHINE_UNKNOWN ||
        nativeMachine != IMAGE_FILE_MACHINE_AMD64) return false;

    APPBARDATA appbar{};
    appbar.cbSize = sizeof(appbar);
    if (!SHAppBarMessage(ABM_GETTASKBARPOS, &appbar) || (appbar.uEdge != ABE_BOTTOM && appbar.uEdge != ABE_TOP)) return false;
    RECT rect{};
    if (!GetWindowRect(taskbar, &rect) || rect.right - rect.left <= rect.bottom - rect.top) return false;
    return true;
}

struct LivePublisherFrame {
    DWORD ownerPid = 0;
    std::uint32_t generation = 0, width = 0, height = 0, dpi = 0, items = 0;
};

bool VerifyLivePublisher(DWORD explorerPid, HWND taskbar, LivePublisherFrame& result,
                         wchar_t* reason, size_t reasonCount) {
    auto fail = [&](wchar_t const* text) {
        wcsncpy_s(reason, reasonCount, text, _TRUNCATE);
        return false;
    };
    wchar_t name[128]{};
    if (swprintf_s(name, L"%s%lu", widget_bridge::kMappingPrefix, explorerPid) < 0)
        return fail(L"mapping-name");
    HANDLE mapping = OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, FALSE, name);
    if (!mapping) return fail(L"BatteryHub publisher mapping missing");
    auto* surface = static_cast<widget_bridge::SharedSurface*>(
        MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, widget_bridge::kMappingBytes));
    if (!surface) {
        CloseHandle(mapping);
        return fail(L"BatteryHub publisher mapping invalid");
    }
    bool ok = widget_bridge::ValidatePeerIdentity(*surface, explorerPid, taskbar, false);
    const LONG requested = InterlockedCompareExchange(
        reinterpret_cast<volatile LONG*>(&surface->requested), 0, 0);
    widget_bridge::SurfaceFrame frame;
    if (!ok) wcsncpy_s(reason, reasonCount, L"BatteryHub/Explorer identity rejected", _TRUNCATE);
    else if (!requested) { ok = false; wcsncpy_s(reason, reasonCount, L"experimental position not requested", _TRUNCATE); }
    else if (!widget_bridge::ReadStableFrame(surface, explorerPid, taskbar, frame)) {
        ok = false;
        wcsncpy_s(reason, reasonCount, L"no stable PaintStrip frame", _TRUNCATE);
    } else if (frame.itemRects.empty()) {
        ok = false;
        wcsncpy_s(reason, reasonCount, L"no visible BatteryHub devices in frame", _TRUNCATE);
    } else if (InterlockedCompareExchange(
                   reinterpret_cast<volatile LONG*>(&surface->integrationState), 0, 0) ==
               static_cast<LONG>(widget_bridge::IntegrationState::Failed)) {
        ok = false;
        wcsncpy_s(reason, reasonCount, L"publisher reports integration failure", _TRUNCATE);
    }
    if (ok) {
        result = { surface->ownerPid, frame.generation, frame.widthPx, frame.heightPx, frame.dpi,
                   static_cast<DWORD>(frame.itemRects.size()) };
    }
    UnmapViewOfFile(surface);
    CloseHandle(mapping);
    return ok;
}

void PrintStatus(wchar_t const* stage, HostTestSharedState const* shared) {
    const LONG state = InterlockedCompareExchange(const_cast<volatile LONG*>(&shared->state), 0, 0);
    wprintf(L"%s state=%ld attach=0x%08X detach=0x%08X\n", stage,
        state,
        static_cast<unsigned>(shared->attachResult), static_cast<unsigned>(shared->detachResult));
}

LONG ReadState(HostTestSharedState const* shared) noexcept {
    return InterlockedCompareExchange(const_cast<volatile LONG*>(&shared->state), 0, 0);
}

bool SendOneMessage(HWND hwnd, UINT message, DWORD timeoutMs, std::wstring const& logPath, wchar_t const* label) {
    DWORD_PTR result = 0;
    SetLastError(ERROR_SUCCESS);
    const LRESULT sent = SendMessageTimeoutW(hwnd, message, 0, 0, SMTO_ABORTIFHUNG | SMTO_BLOCK, timeoutMs, &result);
    const DWORD error = sent ? ERROR_SUCCESS : GetLastError();
    wchar_t detail[256]{};
    swprintf_s(detail, L"sent=%lld error=%lu", static_cast<long long>(sent), error);
    AppendLog(logPath, label, detail);
    return sent != 0;
}

bool StopRequested(HANDLE stop) noexcept {
    return stop && WaitForSingleObject(stop, 0) == WAIT_OBJECT_0;
}

bool WaitForStop(HANDLE stop, DWORD milliseconds) noexcept {
    return stop && WaitForSingleObject(stop, milliseconds) == WAIT_OBJECT_0;
}

std::wstring ExperimentalImagePath() {
    const std::wstring dir = ModuleDirectory();
    if (dir.empty()) return {};
    std::wstring path = dir + L"\\..\\run\\BatteryHub-Experimental.exe";
    std::wstring full(32768, L'\0');
    const DWORD length = GetFullPathNameW(path.c_str(), static_cast<DWORD>(full.size()), full.data(), nullptr);
    if (!length || length >= full.size()) return {};
    full.resize(length);
    return full;
}

bool VerifyExperimentalProcess(HANDLE process, std::wstring const& expected) {
    wchar_t image[32768]{};
    DWORD length = ARRAYSIZE(image);
    return process && QueryFullProcessImageNameW(process, 0, image, &length) && _wcsicmp(image, expected.c_str()) == 0;
}

struct HostSession {
    HANDLE mapping = nullptr;
    HostTestSharedState* shared = nullptr;
    HMODULE adapter = nullptr;
    HHOOK callHook = nullptr;
    HHOOK getMessageHook = nullptr;
    HANDLE explorer = nullptr;
    HWND taskbar = nullptr;
    DWORD explorerPid = 0;
    UINT message = 0;
    std::wstring logPath;

    bool Attach(HWND target, DWORD pid, DWORD tid, HANDLE targetProcess, DWORD ownerPid,
                std::wstring const& adapterPath, std::wstring const& sessionLog) {
        taskbar = target;
        explorerPid = pid;
        explorer = targetProcess;
        logPath = sessionLog;
        wchar_t mappingName[128]{};
        if (swprintf_s(mappingName, L"%s%lu", kHostTestMappingPrefix, pid) < 0) return false;
        SetLastError(ERROR_SUCCESS);
        mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0,
                                     sizeof(HostTestSharedState), mappingName);
        if (!mapping || GetLastError() == ERROR_ALREADY_EXISTS) {
            AppendLog(logPath, L"attach-abort-existing-host-mapping");
            return false;
        }
        shared = static_cast<HostTestSharedState*>(MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0,
                                                                  sizeof(HostTestSharedState)));
        if (!shared) return false;
        ZeroMemory(shared, sizeof(*shared));
        shared->magic = kHostTestMagic;
        shared->version = kHostTestVersion;
        shared->targetPid = pid;
        shared->targetThreadId = tid;
        shared->taskbarWindow = target;
        shared->watchdogMs = kExperimentalWatchdogMs;
        wcscpy_s(shared->logPath, logPath.c_str());

        adapter = LoadLibraryExW(adapterPath.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
        auto hookProc = adapter ? reinterpret_cast<HostTestHookProc>(GetProcAddress(adapter, "BatteryHubTaskbarHookProc")) : nullptr;
        auto getProc = adapter ? reinterpret_cast<HostTestHookProc>(GetProcAddress(adapter, "BatteryHubTaskbarGetMessageHookProc")) : nullptr;
        if (!hookProc || !getProc) { AppendLog(logPath, L"attach-abort-adapter-load"); return false; }

        LivePublisherFrame frame{};
        wchar_t reason[128]{};
        if (!VerifyLivePublisher(pid, target, frame, reason, ARRAYSIZE(reason)) || frame.ownerPid != ownerPid) {
            AppendLog(logPath, L"attach-abort-publisher-changed", reason);
            return false;
        }
        if (WaitForSingleObject(explorer, 0) != WAIT_TIMEOUT || FindWindowW(L"Shell_TrayWnd", nullptr) != target) {
            AppendLog(logPath, L"attach-abort-taskbar-changed");
            return false;
        }
        callHook = SetWindowsHookExW(WH_CALLWNDPROC, hookProc, adapter, tid);
        if (!callHook) { AppendLog(logPath, L"attach-abort-callwndproc-hook", std::to_wstring(GetLastError()).c_str()); return false; }
        getMessageHook = SetWindowsHookExW(WH_GETMESSAGE, getProc, adapter, tid);
        if (!getMessageHook) { AppendLog(logPath, L"attach-abort-getmessage-hook", std::to_wstring(GetLastError()).c_str()); return false; }
        message = RegisterWindowMessageW(kHostTestMessageName);
        if (!message) { AppendLog(logPath, L"attach-abort-register-message", std::to_wstring(GetLastError()).c_str()); return false; }
        AppendLog(logPath, L"service-hooks-installed", L"WH_CALLWNDPROC+WH_GETMESSAGE");
        InterlockedExchange(&shared->state, HostAttachRequested);
        if (!SendOneMessage(taskbar, message, 10000, logPath, L"service-attach-message") ||
            ReadState(shared) != HostAttached) {
            AppendLog(logPath, L"service-attach-failed");
            return false;
        }
        wchar_t detail[128]{};
        swprintf_s(detail, L"explorerPid=%lu publisherPid=%lu watchdogMs=%lu", pid, ownerPid, shared->watchdogMs);
        AppendLog(logPath, L"service-attached", detail);
        return true;
    }

    void Close(bool explorerAlive) noexcept {
        if (explorerAlive && shared && taskbar && message &&
            ReadState(shared) != HostDetached && ReadState(shared) != HostFailed) {
            InterlockedExchange(&shared->state, HostDetachRequested);
            SendOneMessage(taskbar, message, 5000, logPath, L"service-detach-message");
            const ULONGLONG deadline = GetTickCount64() + 5000;
            while (ReadState(shared) != HostDetached && ReadState(shared) != HostFailed && GetTickCount64() < deadline)
                Sleep(50);
        }
        const bool hadHooks = callHook || getMessageHook;
        if (callHook) { UnhookWindowsHookEx(callHook); callHook = nullptr; }
        if (getMessageHook) { UnhookWindowsHookEx(getMessageHook); getMessageHook = nullptr; }
        if (explorerAlive && hadHooks) Sleep(500);
        if (adapter) { FreeLibrary(adapter); adapter = nullptr; }
        if (shared) { UnmapViewOfFile(shared); shared = nullptr; }
        if (mapping) { CloseHandle(mapping); mapping = nullptr; }
        if (explorer) { CloseHandle(explorer); explorer = nullptr; }
        taskbar = nullptr;
        explorerPid = 0;
        message = 0;
    }
};

int RunService(DWORD ownerPid) {
    HANDLE singleton = CreateMutexW(nullptr, TRUE, L"Local\\BatteryHub.Experimental.TaskbarXaml.Service");
    if (!singleton || GetLastError() == ERROR_ALREADY_EXISTS) {
        if (singleton) CloseHandle(singleton);
        return 20;
    }
    HANDLE stop = CreateEventW(nullptr, TRUE, FALSE, kExperimentalStopEvent);
    if (!stop) { CloseHandle(singleton); return 21; }
    ResetEvent(stop);
    HANDLE owner = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, FALSE, ownerPid);
    const std::wstring expectedOwner = ExperimentalImagePath();
    std::wstring logPath = ModuleDirectory() + L"\\BatteryHubExperimentalXamlService.log";
    AppendLog(logPath, L"service-start");
    if (!owner || expectedOwner.empty() || !VerifyExperimentalProcess(owner, expectedOwner)) {
        AppendLog(logPath, L"service-abort-invalid-publisher-process");
        if (owner) CloseHandle(owner);
        CloseHandle(stop); CloseHandle(singleton);
        return 22;
    }
    std::wstring taskbarPath, pdbPath, adapterPath;
    if (!GetExpectedPaths(taskbarPath, pdbPath, adapterPath)) {
        AppendLog(logPath, L"service-abort-artifact-paths");
        CloseHandle(owner); CloseHandle(stop); CloseHandle(singleton); return 23;
    }
    std::wstring previousTaskbarHash, previousPdbHash;
    DWORD checkedExplorer = 0;
    HostSession session;
    bool running = true;
    while (running && !StopRequested(stop) && WaitForSingleObject(owner, 0) == WAIT_TIMEOUT) {
        HWND taskbar = nullptr;
        DWORD pid = 0, tid = 0;
        HANDLE explorer = nullptr;
        if (!VerifyExplorer(taskbar, pid, tid, explorer)) {
            if (explorer) CloseHandle(explorer);
            WaitForStop(stop, 1000);
            continue;
        }
        if (pid != checkedExplorer) {
            checkedExplorer = pid;
            DWORD build = 0, ubr = 0;
            std::wstring display, taskbarHash, pdbHash;
            if (!QueryWindowsVersion(build, ubr, display) ||
                !Sha256(taskbarPath, taskbarHash) || taskbarHash != kExpectedTaskbarHash ||
                !Sha256(pdbPath, pdbHash) || pdbHash != kExpectedPdbHash ||
                !VerifyPdbWithDbgHelp(taskbarPath, pdbPath, ModuleDirectory())) {
                AppendLog(logPath, L"service-preflight-rejected-unsupported-taskbar");
                CloseHandle(explorer);
                running = false;
                break;
            }
            previousTaskbarHash = std::move(taskbarHash);
            previousPdbHash = std::move(pdbHash);
            wchar_t detail[192]{};
            swprintf_s(detail, L"explorerPid=%lu build=%lu.%lu taskbar=%s pdb=%s",
                       pid, build, ubr, previousTaskbarHash.c_str(), previousPdbHash.c_str());
            AppendLog(logPath, L"service-preflight-passed", detail);
        }
        LivePublisherFrame frame{};
        wchar_t reason[128]{};
        if (!VerifyLivePublisher(pid, taskbar, frame, reason, ARRAYSIZE(reason)) || frame.ownerPid != ownerPid) {
            CloseHandle(explorer);
            WaitForStop(stop, 1000); // keep the traditional strip until a real frame appears
            continue;
        }
        if (!session.Attach(taskbar, pid, tid, explorer, ownerPid, adapterPath, logPath)) {
            session.Close(WaitForSingleObject(session.explorer, 0) == WAIT_TIMEOUT);
            AppendLog(logPath, L"service-attach-retry-delayed");
            WaitForStop(stop, 2000);
            continue;
        }
        while (!StopRequested(stop) && WaitForSingleObject(owner, 0) == WAIT_TIMEOUT &&
               WaitForSingleObject(session.explorer, 0) == WAIT_TIMEOUT && ReadState(session.shared) == HostAttached) {
            HANDLE waits[] = { stop, owner, session.explorer };
            WaitForMultipleObjects(ARRAYSIZE(waits), waits, FALSE, 1000);
        }
        const bool explorerAlive = session.explorer && WaitForSingleObject(session.explorer, 0) == WAIT_TIMEOUT;
        const bool shouldExit = StopRequested(stop) || WaitForSingleObject(owner, 0) == WAIT_OBJECT_0;
        AppendLog(logPath, explorerAlive ? L"service-session-detached" : L"service-explorer-restarted");
        session.Close(explorerAlive);
        if (shouldExit) running = false;
        else WaitForStop(stop, 1000);
    }
    if (session.mapping || session.adapter || session.callHook || session.getMessageHook) {
        const bool explorerAlive = session.explorer && WaitForSingleObject(session.explorer, 0) == WAIT_TIMEOUT;
        session.Close(explorerAlive);
    }
    AppendLog(logPath, StopRequested(stop) ? L"service-stopped" : L"service-publisher-exited");
    CloseHandle(owner); CloseHandle(stop); ReleaseMutex(singleton); CloseHandle(singleton);
    return 0;
}

int StopService() {
    HANDLE stop = OpenEventW(EVENT_MODIFY_STATE, FALSE, kExperimentalStopEvent);
    if (!stop) return GetLastError() == ERROR_FILE_NOT_FOUND ? 0 : 24;
    const BOOL signaled = SetEvent(stop);
    CloseHandle(stop);
    return signaled ? 0 : 25;
}
}

int wmain(int argc, wchar_t** argv) {
    if (argc == 2 && _wcsicmp(argv[1], L"--stop") == 0) return StopService();
    if (argc == 3 && _wcsicmp(argv[1], L"--serve") == 0) {
        wchar_t* end = nullptr;
        const unsigned long pid = wcstoul(argv[2], &end, 10);
        if (!end || *end || !pid || pid > MAXDWORD) return 26;
        return RunService(static_cast<DWORD>(pid));
    }
    SetConsoleCtrlHandler(ConsoleControl, TRUE);
    SetConsoleTitleW(L"BatteryHub temporary taskbar XAML test");
    std::wstring taskbarPath, pdbPath, adapterPath;
    if (!GetExpectedPaths(taskbarPath, pdbPath, adapterPath)) {
        fwprintf(stderr, L"Could not resolve test artifact paths.\n");
        return 2;
    }
    SYSTEMTIME now{};
    GetLocalTime(&now);
    wchar_t logName[96]{};
    swprintf_s(logName, L"BatteryHubTaskbarHostTest-%04u%02u%02u-%02u%02u%02u.log",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond);
    const std::wstring logPath = ModuleDirectory() + L"\\" + logName;
    AppendLog(logPath, L"controller-start");

    DWORD build = 0, ubr = 0;
    std::wstring displayVersion;
    if (!QueryWindowsVersion(build, ubr, displayVersion)) {
        AppendLog(logPath, L"abort-windows-version-mismatch");
        fwprintf(stderr, L"ABORT: this build only permits Windows 11 26H2 build 26300.9818.\nLog: %s\n", logPath.c_str());
        return 3;
    }
    wprintf(L"Windows 11 %s, build %lu.%lu, x64\n", displayVersion.c_str(), build, ubr);
    wchar_t buildInfo[128]{};
    swprintf_s(buildInfo, L"displayVersion=%s build=%lu.%lu architecture=x64",
               displayVersion.c_str(), build, ubr);
    AppendLog(logPath, L"windows-build-verified", buildInfo);

    std::wstring imageHash, pdbHash;
    if (!Sha256(taskbarPath, imageHash) || imageHash != kExpectedTaskbarHash ||
        !Sha256(pdbPath, pdbHash) || pdbHash != kExpectedPdbHash) {
        AppendLog(logPath, L"abort-file-hash-mismatch");
        fwprintf(stderr, L"ABORT: taskbar.dll or local PDB hash differs from the verified build.\nLog: %s\n", logPath.c_str());
        return 4;
    }
    wprintf(L"taskbar.dll SHA256: %s\n", imageHash.c_str());
    wprintf(L"Taskbar.pdb SHA256: %s\n", pdbHash.c_str());
    const std::wstring hashes = L"taskbarSha256=" + imageHash + L" pdbSha256=" + pdbHash;
    AppendLog(logPath, L"image-hashes-verified", hashes.c_str());
    if (!VerifyPdbWithDbgHelp(taskbarPath, pdbPath, ModuleDirectory())) {
        AppendLog(logPath, L"abort-pdb-not-matched");
        fwprintf(stderr, L"ABORT: DbgHelp did not match the PDB identity. No hook installed.\nLog: %s\n", logPath.c_str());
        return 5;
    }
    AppendLog(logPath, L"pdb-identity-verified",
              L"guid=8645A6F8-C08F-C8A1-8DAC-D4B77CF1E2EE age=1");

    HWND taskbar = nullptr;
    DWORD explorerPid = 0, explorerTid = 0;
    HANDLE explorerProcess = nullptr;
    if (!VerifyExplorer(taskbar, explorerPid, explorerTid, explorerProcess)) {
        if (explorerProcess) CloseHandle(explorerProcess);
        AppendLog(logPath, L"abort-explorer-or-taskbar-precheck");
        fwprintf(stderr, L"ABORT: Explorer identity, bitness, or horizontal primary taskbar check failed.\nLog: %s\n", logPath.c_str());
        return 6;
    }
    wchar_t explorerInfo[160]{};
    swprintf_s(explorerInfo, L"explorerPid=%lu thread=%lu hwnd=0x%p image=%s x64=1 primaryHorizontal=1",
               explorerPid, explorerTid, taskbar, L"%windir%\\explorer.exe");
    AppendLog(logPath, L"explorer-verified", explorerInfo);
    LivePublisherFrame publisher{};
    wchar_t publisherError[128]{};
    if (!VerifyLivePublisher(explorerPid, taskbar, publisher, publisherError, ARRAYSIZE(publisherError))) {
        AppendLog(logPath, L"abort-live-publisher", publisherError);
        CloseHandle(explorerProcess);
        fwprintf(stderr, L"ABORT: %s. Start the single experimental BatteryHub publisher and retry only after new approval.\nLog: %s\n",
                 publisherError, logPath.c_str());
        return 7;
    }
    wchar_t info[384]{};
    swprintf_s(info, L"explorerPid=%lu thread=%lu hwnd=0x%p publisherPid=%lu generation=%lu frame=%lux%lu dpi=%lu items=%lu",
        explorerPid, explorerTid, taskbar, publisher.ownerPid, publisher.generation,
        publisher.width, publisher.height, publisher.dpi, publisher.items);
    AppendLog(logPath, L"preflight-passed", info);
    wprintf(L"Explorer PID %lu, UI thread %lu, primary horizontal taskbar verified.\n", explorerPid, explorerTid);
    wprintf(L"BatteryHub publisher PID %lu: live frame %lux%lu px, %lu DPI, %lu devices (generation %lu).\n",
        publisher.ownerPid, publisher.width, publisher.height, publisher.dpi, publisher.items, publisher.generation);
    wprintf(L"After LOAD this controller attaches the live PaintStrip surface, hides the traditional widget only after XAML is ready, and starts a 60 s auto-detach timer. No HID queries run in the adapter.\n");
    wprintf(L"Type LOAD and press Enter to arm the one-time hook, or anything else to abort: ");
    wchar_t answer[32]{};
    if (wscanf_s(L"%31s", answer, static_cast<unsigned>(_countof(answer))) != 1 || _wcsicmp(answer, L"LOAD") != 0) {
        AppendLog(logPath, L"user-aborted-before-hook");
        CloseHandle(explorerProcess);
        return 0;
    }

    const UINT message = RegisterWindowMessageW(kHostTestMessageName);
    if (!message) {
        AppendLog(logPath, L"abort-register-message", std::to_wstring(GetLastError()).c_str());
        CloseHandle(explorerProcess);
        return 7;
    }
    wchar_t mappingName[128]{};
    swprintf_s(mappingName, L"%s%lu", kHostTestMappingPrefix, explorerPid);
    SetLastError(ERROR_SUCCESS);
    HANDLE mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0,
        sizeof(HostTestSharedState), mappingName);
    if (!mapping || GetLastError() == ERROR_ALREADY_EXISTS) {
        if (mapping) CloseHandle(mapping);
        AppendLog(logPath, L"abort-existing-session");
        CloseHandle(explorerProcess);
        fwprintf(stderr, L"ABORT: another test mapping exists for this Explorer PID.\nLog: %s\n", logPath.c_str());
        return 8;
    }
    auto* shared = static_cast<HostTestSharedState*>(MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(HostTestSharedState)));
    if (!shared) {
        CloseHandle(mapping);
        AppendLog(logPath, L"abort-map-view", std::to_wstring(GetLastError()).c_str());
        CloseHandle(explorerProcess);
        return 9;
    }
    ZeroMemory(shared, sizeof(*shared));
    shared->magic = kHostTestMagic;
    shared->version = kHostTestVersion;
    shared->targetPid = explorerPid;
    shared->targetThreadId = explorerTid;
    shared->taskbarWindow = taskbar;
    wcscpy_s(shared->logPath, logPath.c_str());

    HMODULE adapter = LoadLibraryExW(adapterPath.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    auto hookProc = adapter ? reinterpret_cast<HostTestHookProc>(GetProcAddress(adapter, "BatteryHubTaskbarHookProc")) : nullptr;
    auto getMessageHookProc = adapter ? reinterpret_cast<HostTestHookProc>(
        GetProcAddress(adapter, "BatteryHubTaskbarGetMessageHookProc")) : nullptr;
    if (!hookProc || !getMessageHookProc) {
        AppendLog(logPath, L"abort-adapter-load", std::to_wstring(GetLastError()).c_str());
        if (adapter) FreeLibrary(adapter);
        UnmapViewOfFile(shared);
        CloseHandle(mapping);
        CloseHandle(explorerProcess);
        return 10;
    }

    LivePublisherFrame publisherAtLoad{};
    const bool explorerStable = WaitForSingleObject(explorerProcess, 0) == WAIT_TIMEOUT &&
                                FindWindowW(L"Shell_TrayWnd", nullptr) == taskbar;
    const bool publisherStable = explorerStable && VerifyLivePublisher(
        explorerPid, taskbar, publisherAtLoad, publisherError, ARRAYSIZE(publisherError));
    if (!publisherStable) {
        if (!explorerStable) wcsncpy_s(publisherError, ARRAYSIZE(publisherError), L"Explorer taskbar changed", _TRUNCATE);
        AppendLog(logPath, L"abort-preconditions-changed-before-hook", publisherError);
        FreeLibrary(adapter);
        UnmapViewOfFile(shared);
        CloseHandle(mapping);
        CloseHandle(explorerProcess);
        fwprintf(stderr, L"ABORT: %s; no hook was installed.\nLog: %s\n", publisherError, logPath.c_str());
        return 11;
    }
    HHOOK hook = SetWindowsHookExW(WH_CALLWNDPROC, hookProc, adapter, explorerTid);
    if (!hook) {
        wchar_t detail[80]{};
        swprintf_s(detail, L"error=%lu", GetLastError());
        AppendLog(logPath, L"abort-hook-install", detail);
        FreeLibrary(adapter);
        UnmapViewOfFile(shared);
        CloseHandle(mapping);
        CloseHandle(explorerProcess);
        return 12;
    }
    HHOOK getMessageHook = SetWindowsHookExW(WH_GETMESSAGE, getMessageHookProc, adapter, explorerTid);
    if (!getMessageHook) {
        wchar_t detail[80]{};
        swprintf_s(detail, L"error=%lu", GetLastError());
        AppendLog(logPath, L"abort-getmessage-hook-install", detail);
        UnhookWindowsHookEx(hook);
        FreeLibrary(adapter);
        UnmapViewOfFile(shared);
        CloseHandle(mapping);
        CloseHandle(explorerProcess);
        return 12;
    }
    AppendLog(logPath, L"hooks-installed-once", L"WH_CALLWNDPROC+WH_GETMESSAGE");
    InterlockedExchange(&shared->state, HostAttachRequested);
    if (!SendOneMessage(taskbar, message, 10000, logPath, L"attach-message")) {
        if (InterlockedCompareExchange(&shared->state, HostFailed, HostAttachRequested) == HostAttachRequested)
            AppendLog(logPath, L"attach-cancelled-before-dispatch");
        else AppendLog(logPath, L"attach-message-timeout-no-retry");
    }
    PrintStatus(L"after insert request:", shared);

    if (ReadState(shared) == HostAttached) {
        wprintf(L"Block inserted. Capture the taskbar now; press D once to remove, or leave it for the 60 s watchdog.\n");
        const ULONGLONG controllerDetachAt = GetTickCount64() + kHostTestWatchdogMs;
        while (!g_cancel.load() && ReadState(shared) == HostAttached) {
            if (WaitForSingleObject(explorerProcess, 250) == WAIT_OBJECT_0) {
                AppendLog(logPath, L"explorer-exited-stop-no-retry");
                wprintf(L"Explorer exited. Test stopped; no retry will occur.\n");
                break;
            }
            if (_kbhit()) {
                int key = _getch();
                if (key == 'd' || key == 'D') {
                    InterlockedCompareExchange(&shared->state, HostDetachRequested, HostAttached);
                    SendOneMessage(taskbar, message, 5000, logPath, L"manual-detach-message");
                    break;
                }
            }
            if (GetTickCount64() >= controllerDetachAt) {
                AppendLog(logPath, L"controller-watchdog-detach-request");
                if (InterlockedCompareExchange(&shared->state, HostDetachRequested, HostAttached) == HostAttached)
                    SendOneMessage(taskbar, message, 5000, logPath, L"controller-watchdog-detach-message");
                break;
            }
            Sleep(100);
        }
    }
    if (g_cancel.load() && ReadState(shared) == HostAttached) {
        InterlockedCompareExchange(&shared->state, HostDetachRequested, HostAttached);
        SendOneMessage(taskbar, message, 5000, logPath, L"ctrl-detach-message");
    }

    const ULONGLONG finishDeadline = GetTickCount64() + kHostTestWatchdogMs + 5000;
    while (WaitForSingleObject(explorerProcess, 0) == WAIT_TIMEOUT &&
           ReadState(shared) != HostDetached && ReadState(shared) != HostFailed && GetTickCount64() < finishDeadline) Sleep(100);
    PrintStatus(L"final:", shared);
    if (ReadState(shared) != HostDetached && ReadState(shared) != HostFailed && WaitForSingleObject(explorerProcess, 0) == WAIT_TIMEOUT)
        AppendLog(logPath, L"detach-not-confirmed-stop-no-retry");
    if (WaitForSingleObject(explorerProcess, 0) == WAIT_TIMEOUT) {
        Sleep(500); // Let a completed hook callback return before unhooking it.
        if (!UnhookWindowsHookEx(hook)) AppendLog(logPath, L"unhook-failed", std::to_wstring(GetLastError()).c_str());
        else AppendLog(logPath, L"hook-removed");
        if (!UnhookWindowsHookEx(getMessageHook)) AppendLog(logPath, L"getmessage-unhook-failed", std::to_wstring(GetLastError()).c_str());
        else AppendLog(logPath, L"getmessage-hook-removed");
    }
    FreeLibrary(adapter);
    UnmapViewOfFile(shared);
    CloseHandle(mapping);
    CloseHandle(explorerProcess);
    wprintf(L"Log: %s\n", logPath.c_str());
    return 0;
}
