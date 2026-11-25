#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#undef GetCurrentTime
#include <unknwn.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.Xaml.h>

#include "HostTestProtocol.h"
#include "../taskbar-xaml-vm/TaskbarXamlModule.h"
#include "../taskbar-xaml-widget/WidgetSurfaceReader.h"
#include "../taskbar-xaml-widget/WidgetSurfaceView.h"

#include <cstdint>
#include <cstdio>
#include <cwchar>
#include <new>
#include <string>

using namespace winrt;
using namespace winrt::Windows::UI::Xaml;

namespace {
constexpr DWORD64 kPrimaryVtableRva = 0x1CFA30;
constexpr DWORD64 kGetTaskbarHostRva = 0x1237B0;
constexpr DWORD64 kFrameHeightRva = 0xB10A4;
constexpr DWORD64 kDecrefRva = 0x290B0;
constexpr BYTE kGetTaskbarHostPrologue[] = {0x40, 0x53, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x8B, 0x89, 0xE8, 0x01, 0x00, 0x00};
constexpr BYTE kFrameHeightPrologue[] = {0x48, 0x83, 0xEC, 0x28, 0x48, 0x83, 0xC1, 0x10};
constexpr wchar_t kTrayWindowClass[] = L"Shell_TrayWnd";
constexpr UINT_PTR kSurfaceHeartbeatTimer = 0xB411;
constexpr UINT kDeliverFrameMessage = WM_APP + 0x35B;
constexpr UINT kDetachSurfaceMessage = WM_APP + 0x35C;
bool RefreshWidgetSurface(HWND taskbarWindow) noexcept;
void ResetWidgetSurface(bool detachXaml) noexcept;
void StartSurfaceReader(HWND taskbarWindow) noexcept;
void StopSurfaceReader() noexcept;
std::wstring g_hostLogPath;
SRWLOCK g_adapterLogLock = SRWLOCK_INIT;

void AdapterTrace(wchar_t const* stage, HRESULT hr = S_OK,
                  widget_bridge::SurfaceFrame const* frame = nullptr) noexcept {
    if (g_hostLogPath.empty()) return;
    SYSTEMTIME now{};
    GetSystemTime(&now);
    const auto session = frame ? frame->sessionId : 0;
    const auto generation = frame ? frame->generation : 0;
    const auto width = frame ? frame->widthPx : 0;
    const auto height = frame ? frame->heightPx : 0;
    const auto dpi = frame ? frame->dpi : 0;
    const auto bytes = frame ? static_cast<std::uint64_t>(frame->pixels.size()) * sizeof(std::uint32_t) : 0;
    const auto hash = frame ? widget_bridge::PixelHash(frame->pixels.data(), frame->pixels.size()) : 0;
    wchar_t line[768]{};
    const int chars = swprintf_s(line,
        L"{\"tsUtc\":\"%04u-%02u-%02uT%02u:%02u:%02u.%03uZ\",\"pid\":%lu,\"tid\":%lu,"
        L"\"session\":\"%016llX\",\"frame\":%lu,\"stage\":\"%s\",\"hr\":\"0x%08X\","
        L"\"widthPx\":%lu,\"heightPx\":%lu,\"dpi\":%lu,\"stride\":%lu,\"pixelBytes\":%llu,"
        L"\"format\":\"BGRA32-premultiplied\",\"pixelHash\":\"%016llX\"}\r\n",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds,
        GetCurrentProcessId(), GetCurrentThreadId(), static_cast<unsigned long long>(session), generation,
        stage, static_cast<unsigned>(hr), width, height, dpi, width * 4u,
        static_cast<unsigned long long>(bytes), static_cast<unsigned long long>(hash));
    if (chars <= 0 || static_cast<std::size_t>(chars) >= ARRAYSIZE(line)) return;
    char utf8[sizeof(line) * 4]{};
    const int byteCount = WideCharToMultiByte(CP_UTF8, 0, line, chars, utf8,
        static_cast<int>(sizeof(utf8)), nullptr, nullptr);
    if (byteCount <= 0) return;
    AcquireSRWLockExclusive(&g_adapterLogLock);
    HANDLE file = CreateFileW(g_hostLogPath.c_str(), FILE_APPEND_DATA,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_ALWAYS,
        FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file != INVALID_HANDLE_VALUE) {
        DWORD written = 0;
        WriteFile(file, utf8, static_cast<DWORD>(byteCount), &written, nullptr);
        CloseHandle(file);
    }
    ReleaseSRWLockExclusive(&g_adapterLogLock);
}

struct SharedView {
    HANDLE mapping = nullptr;
    HostTestSharedState* state = nullptr;
    ~SharedView() {
        if (state) UnmapViewOfFile(state);
        if (mapping) CloseHandle(mapping);
    }
};

bool IsReadable(void const* address, SIZE_T size) noexcept {
    if (!address || !size) return false;
    MEMORY_BASIC_INFORMATION mbi{};
    if (!VirtualQuery(address, &mbi, sizeof(mbi)) || mbi.State != MEM_COMMIT ||
        (mbi.Protect & (PAGE_GUARD | PAGE_NOACCESS))) return false;
    const auto start = reinterpret_cast<std::uintptr_t>(address);
    const auto base = reinterpret_cast<std::uintptr_t>(mbi.BaseAddress);
    const auto end = base + mbi.RegionSize;
    return start >= base && start <= end && size <= end - start;
}

bool OpenShared(SharedView& view) noexcept {
    wchar_t name[128]{};
    swprintf_s(name, L"%s%lu", kHostTestMappingPrefix, GetCurrentProcessId());
    view.mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, name);
    if (!view.mapping) return false;
    view.state = static_cast<HostTestSharedState*>(MapViewOfFile(view.mapping, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(HostTestSharedState)));
    if (!view.state || view.state->magic != kHostTestMagic || view.state->version != kHostTestVersion ||
        view.state->targetPid != GetCurrentProcessId()) return false;
    return true;
}

void Log(HostTestSharedState const* state, wchar_t const* event, HRESULT hr = S_OK) noexcept {
    SYSTEMTIME now{};
    GetSystemTime(&now);
    wchar_t line[512]{};
    swprintf_s(line, L"%04u-%02u-%02uT%02u:%02u:%02u.%03uZ pid=%lu tid=%lu event=%s hr=0x%08X\r\n",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds,
        GetCurrentProcessId(), GetCurrentThreadId(), event, static_cast<unsigned>(hr));
    OutputDebugStringW(line);
    if (!state || !state->logPath[0]) return;
    HANDLE file = CreateFileW(state->logPath, FILE_APPEND_DATA,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return;
    char utf8[sizeof(line) * 4]{};
    const int encodedBytes = WideCharToMultiByte(CP_UTF8, 0, line, -1, utf8, static_cast<int>(sizeof(utf8)), nullptr, nullptr);
    DWORD written = 0;
    if (encodedBytes > 1) WriteFile(file, utf8, static_cast<DWORD>(encodedBytes - 1), &written, nullptr);
    CloseHandle(file);
}

HRESULT GetTaskbarRoot(HWND taskbarWindow, UIElement& rootContent) noexcept {
    if (!taskbarWindow || !IsWindow(taskbarWindow)) return E_HANDLE;
    wchar_t className[64]{};
    if (!GetClassNameW(taskbarWindow, className, ARRAYSIZE(className)) || _wcsicmp(className, kTrayWindowClass) != 0)
        return HRESULT_FROM_WIN32(ERROR_INVALID_WINDOW_HANDLE);
    DWORD ownerPid = 0;
    const DWORD ownerTid = GetWindowThreadProcessId(taskbarWindow, &ownerPid);
    if (!ownerTid || ownerTid != GetCurrentThreadId() || ownerPid != GetCurrentProcessId())
        return HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED);

    HMODULE taskbar = GetModuleHandleW(L"taskbar.dll");
    if (!taskbar) return HRESULT_FROM_WIN32(ERROR_MOD_NOT_FOUND);
    auto const* const module = reinterpret_cast<BYTE const*>(taskbar);
    auto const* const hostPrologue = module + kGetTaskbarHostRva;
    auto const* const framePrologue = module + kFrameHeightRva;
    if (!IsReadable(hostPrologue, sizeof(kGetTaskbarHostPrologue)) ||
        memcmp(hostPrologue, kGetTaskbarHostPrologue, sizeof(kGetTaskbarHostPrologue)) != 0 ||
        !IsReadable(framePrologue, sizeof(kFrameHeightPrologue)) ||
        memcmp(framePrologue, kFrameHeightPrologue, sizeof(kFrameHeightPrologue)) != 0)
        return HRESULT_FROM_WIN32(ERROR_REVISION_MISMATCH);

    HWND taskbandWindow = reinterpret_cast<HWND>(GetPropW(taskbarWindow, L"TaskbandHWND"));
    if (!taskbandWindow || !IsWindow(taskbandWindow)) return HRESULT_FROM_WIN32(ERROR_NOT_FOUND);
    DWORD taskbandPid = 0;
    const DWORD taskbandTid = GetWindowThreadProcessId(taskbandWindow, &taskbandPid);
    if (taskbandPid != GetCurrentProcessId() || taskbandTid != GetCurrentThreadId())
        return HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED);
    void* taskBand = reinterpret_cast<void*>(GetWindowLongPtrW(taskbandWindow, 0));
    if (!IsReadable(taskBand, sizeof(void*))) return HRESULT_FROM_WIN32(ERROR_INVALID_ADDRESS);

    void* const expectedVtable = const_cast<BYTE*>(module + kPrimaryVtableRva);
    void* taskBandInterface = nullptr;
    auto* candidate = static_cast<void**>(taskBand);
    for (unsigned i = 0; i <= 20; ++i) {
        if (!IsReadable(candidate + i, sizeof(void*))) return HRESULT_FROM_WIN32(ERROR_INVALID_ADDRESS);
        if (candidate[i] == expectedVtable) {
            taskBandInterface = candidate + i;
            break;
        }
    }
    if (!taskBandInterface) return HRESULT_FROM_WIN32(ERROR_NOT_FOUND);

    using GetTaskbarHostFn = void (WINAPI*)(void*, void*);
    using DecrefFn = void (WINAPI*)(void*);
    auto getTaskbarHost = reinterpret_cast<GetTaskbarHostFn>(module + kGetTaskbarHostRva);
    auto decref = reinterpret_cast<DecrefFn>(module + kDecrefRva);
    alignas(void*) void* shared[2]{};
    getTaskbarHost(taskBandInterface, shared);
    if (!shared[0] || !shared[1]) {
        if (shared[1]) decref(shared[1]);
        return HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
    }

    HRESULT hr = E_FAIL;
    try {
        auto* hostElementSlot = reinterpret_cast<IUnknown**>(static_cast<BYTE*>(shared[0]) + 0x10);
        if (!IsReadable(hostElementSlot, sizeof(*hostElementSlot)) || !*hostElementSlot) {
            hr = HRESULT_FROM_WIN32(ERROR_INVALID_ADDRESS);
        } else {
            FrameworkElement taskbarElement{nullptr};
            hr = (*hostElementSlot)->QueryInterface(guid_of<FrameworkElement>(), put_abi(taskbarElement));
            if (SUCCEEDED(hr) && taskbarElement) {
                auto xamlRoot = taskbarElement.XamlRoot();
                if (!xamlRoot) hr = HRESULT_FROM_WIN32(ERROR_NOT_FOUND);
                else {
                    rootContent = xamlRoot.Content();
                    if (!rootContent) hr = HRESULT_FROM_WIN32(ERROR_NOT_FOUND);
                }
            }
        }
    } catch (hresult_error const& e) {
        hr = e.code();
    } catch (...) {
        hr = E_FAIL;
    }
    decref(shared[1]);
    return hr;
}

HRESULT Attach(HostTestSharedState* state) noexcept {
    try { g_hostLogPath.assign(state->logPath); } catch (...) { g_hostLogPath.clear(); }
    HRESULT hr = BatteryHubTaskbarXamlSetLogPath(state->logPath);
    if (FAILED(hr)) return hr;
    UIElement root{nullptr};
    hr = GetTaskbarRoot(state->taskbarWindow, root);
    if (FAILED(hr)) return hr;
    const DWORD watchdogMs = state->watchdogMs >= kHostTestWatchdogMs &&
                             state->watchdogMs <= kExperimentalWatchdogMs
        ? state->watchdogMs : kHostTestWatchdogMs;
    state->timerId = SetTimer(state->taskbarWindow, 0, watchdogMs, nullptr);
    if (!state->timerId)
        return HRESULT_FROM_WIN32(GetLastError());
    hr = BatteryHubTaskbarXamlAttachVerifiedTaskbarTree(get_abi(root), state->taskbarWindow);
    if (FAILED(hr)) {
        KillTimer(state->taskbarWindow, state->timerId);
        state->timerId = 0;
        BatteryHubTaskbarXamlDetach();
        return hr;
    }
    AdapterTrace(L"xaml-tree-attached", hr);
    StartSurfaceReader(state->taskbarWindow);
    SetTimer(state->taskbarWindow, kSurfaceHeartbeatTimer, 1000, nullptr);
    return hr;
}

HRESULT Detach(HostTestSharedState* state) noexcept {
    if (state && state->taskbarWindow && state->timerId) KillTimer(state->taskbarWindow, state->timerId);
    if (state && state->taskbarWindow) KillTimer(state->taskbarWindow, kSurfaceHeartbeatTimer);
    if (state) state->timerId = 0;
    StopSurfaceReader();
    ResetWidgetSurface(false);
    return BatteryHubTaskbarXamlDetach();
}

UINT MessageId() noexcept {
    static const UINT id = RegisterWindowMessageW(kHostTestMessageName);
    return id;
}

UINT SurfaceMessageId() noexcept {
    static const UINT id = RegisterWindowMessageW(widget_bridge::kSurfaceChangedMessage);
    return id;
}

struct WidgetMapping {
    HANDLE handle = nullptr;
    widget_bridge::SharedSurface* surface = nullptr;
    ~WidgetMapping() {
        if (surface) UnmapViewOfFile(surface);
        if (handle) CloseHandle(handle);
    }
    void Close() noexcept {
        if (surface) UnmapViewOfFile(surface);
        if (handle) CloseHandle(handle);
        surface = nullptr;
        handle = nullptr;
    }
    bool Open() noexcept {
        if (surface) return true;
        wchar_t name[128]{};
        swprintf_s(name, L"%s%lu", widget_bridge::kMappingPrefix, GetCurrentProcessId());
        handle = OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, FALSE, name);
        if (!handle) return false;
        surface = static_cast<widget_bridge::SharedSurface*>(
            MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, widget_bridge::kMappingBytes));
        if (!surface || surface->magic != widget_bridge::kMagic || surface->version != widget_bridge::kVersion ||
            surface->targetPid != GetCurrentProcessId()) {
            if (surface) UnmapViewOfFile(surface);
            surface = nullptr;
            CloseHandle(handle);
            handle = nullptr;
            return false;
        }
        return true;
    }
};

SRWLOCK g_pendingLock = SRWLOCK_INIT;
SRWLOCK g_slotLock = SRWLOCK_INIT;
widget_bridge::SurfaceFrame g_pendingFrame;
RECT g_uiSlot{};
bool g_pendingReady = false;
volatile LONG g_uiSlotValid = 0;
volatile LONG g_readerEnabled = 0;
volatile LONG g_requestedByApp = 0;
volatile LONG g_peerValid = 0;
volatile LONG g_haveRenderedFrame = 0;
volatile LONG g_slotGeometryFailures = 0;
struct ReaderContext { HWND taskbar; HANDLE wake, stop, thread; HMODULE module; };
ReaderContext* g_readerContext = nullptr;
std::vector<ReaderContext*> g_retiredReaders;

void SetSharedState(widget_bridge::SharedSurface* surface, widget_bridge::IntegrationState state,
                    RECT const* slot) noexcept {
    if (!surface) return;
    auto* sequence = reinterpret_cast<volatile LONG*>(&surface->slotSequence);
    InterlockedIncrement(sequence);
    surface->slotScreenRect = slot ? widget_bridge::Rect{ slot->left, slot->top, slot->right, slot->bottom }
                                   : widget_bridge::Rect{};
    MemoryBarrier();
    InterlockedIncrement(sequence);
    InterlockedExchange64(reinterpret_cast<volatile LONG64*>(&surface->integrationHeartbeatMs),
                          static_cast<LONG64>(GetTickCount64()));
    InterlockedExchange(reinterpret_cast<volatile LONG*>(&surface->integrationState), static_cast<LONG>(state));
}

void ResetWidgetSurface(bool detachXaml) noexcept {
    StopSurfaceReader();
    InterlockedExchange(&g_uiSlotValid, 0);
    InterlockedExchange(&g_haveRenderedFrame, 0);
    if (TryAcquireSRWLockExclusive(&g_pendingLock)) {
        g_pendingReady = false;
        g_pendingFrame = {};
        ReleaseSRWLockExclusive(&g_pendingLock);
    }
    if (detachXaml) {
        BatteryHubTaskbarXamlSetWidgetVisible(FALSE);
        BatteryHubTaskbarXamlDetach();
    }
}

DWORD WINAPI ReadSurfaceFrames(void* parameter) noexcept {
    auto* context = static_cast<ReaderContext*>(parameter);
    HWND taskbarWindow = context->taskbar;
    WidgetMapping mapping;
    bool peerValidated = false, wasRequested = false;
    std::uint64_t lastSessionId = 0, lastReadySessionId = 0;
    std::uint32_t lastGeneration = 0, lastReadyGeneration = 0;
    unsigned invalidReads = 0;
    bool lastRequested = false, lastReady = false, mappingLogged = false;
    HANDLE events[2]{ context->stop, context->wake };
    while (context->stop && context->wake) {
        DWORD wait = WaitForMultipleObjects(2, events, FALSE, 1000);
        if (wait == WAIT_OBJECT_0 || wait == WAIT_FAILED) break;
        if (InterlockedCompareExchange(&g_readerEnabled, 0, 0) == 0) {
            break;
        }
        if (!mapping.Open()) continue;
        if (!mappingLogged) { AdapterTrace(L"mapping-opened"); mappingLogged = true; }
        auto* surface = mapping.surface;
        if (!peerValidated) {
            if (!widget_bridge::ValidatePeerIdentity(*surface, GetCurrentProcessId(), taskbarWindow)) {
                AdapterTrace(L"publisher-authentication-rejected", HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED));
                mapping.Close();
                mappingLogged = false;
                continue;
            }
            peerValidated = true;
            InterlockedExchange(&g_peerValid, 1);
            widget_bridge::SurfaceFrame identity{};
            identity.sessionId = surface->sessionId;
            AdapterTrace(L"publisher-authenticated", S_OK, &identity);
        }
        if (!surface->ownerWindow || !IsWindow(reinterpret_cast<HWND>(surface->ownerWindow))) {
            if (wasRequested && IsWindow(taskbarWindow)) PostMessageW(taskbarWindow, kDetachSurfaceMessage, 0, 0);
            SetSharedState(surface, widget_bridge::IntegrationState::Failed, nullptr);
            InterlockedExchange(&g_peerValid, 0);
            mapping.Close();
            mappingLogged = false;
            peerValidated = false;
            wasRequested = false;
            lastRequested = false;
            lastReady = false;
            lastSessionId = 0;
            lastGeneration = 0;
            continue;
        }
        const bool requested = InterlockedCompareExchange(
            reinterpret_cast<volatile LONG*>(&surface->requested), 0, 0) != 0;
        InterlockedExchange(&g_requestedByApp, requested ? 1 : 0);
        if (requested != lastRequested) {
            AdapterTrace(requested ? L"publisher-requested" : L"publisher-request-cleared",
                         S_OK, nullptr);
            lastRequested = requested;
        }
        if (!requested) {
            SetSharedState(surface, widget_bridge::IntegrationState::Traditional, nullptr);
            InterlockedExchange(&g_uiSlotValid, 0);
            if (wasRequested && IsWindow(taskbarWindow)) PostMessageW(taskbarWindow, kDetachSurfaceMessage, 0, 0);
            wasRequested = false;
            lastReady = false;
            lastSessionId = 0;
            lastGeneration = 0;
            continue;
        }
        wasRequested = true;

        widget_bridge::SurfaceFrame frame;
        bool changed = false;
        if (!widget_bridge::ReadStableFrameIfChanged(surface, GetCurrentProcessId(), taskbarWindow,
                                                      lastSessionId, lastGeneration, frame, changed)) {
            if (++invalidReads >= 3) {
                AdapterTrace(L"frame-validation-failed-three-times", HRESULT_FROM_WIN32(ERROR_INVALID_DATA));
                SetSharedState(surface, widget_bridge::IntegrationState::Failed, nullptr);
                if (IsWindow(taskbarWindow)) PostMessageW(taskbarWindow, kDetachSurfaceMessage, 0, 0);
                InterlockedExchange(&g_peerValid, 0);
                peerValidated = false;
                invalidReads = 0;
            }
            continue; // a transient odd seqlock sequence is retried on the next signal/tick
        }
        invalidReads = 0;
        if (changed) {
            lastSessionId = frame.sessionId;
            lastGeneration = frame.generation;
            AdapterTrace(L"frame-read-stable", S_OK, &frame);
            AcquireSRWLockExclusive(&g_pendingLock);
            g_pendingFrame = std::move(frame);
            g_pendingReady = true;
            ReleaseSRWLockExclusive(&g_pendingLock);
            if (IsWindow(taskbarWindow) && PostMessageW(taskbarWindow, kDeliverFrameMessage, 0, 0))
                AdapterTrace(L"frame-delivery-posted", S_OK, &frame);
            else
                AdapterTrace(L"frame-delivery-post-failed", HRESULT_FROM_WIN32(GetLastError()), &frame);
        }

        RECT slot{};
        bool slotValid = false;
        if (InterlockedCompareExchange(&g_uiSlotValid, 0, 0) != 0) {
            AcquireSRWLockShared(&g_slotLock);
            slot = g_uiSlot;
            ReleaseSRWLockShared(&g_slotLock);
            // UI code commits this slot only after XAML reports the presented geometry.
            // Keep the last committed slot authoritative while the replacement generation is
            // attached; comparing it with the incoming frame temporarily restores the legacy HWND.
            slotValid = slot.right > slot.left && slot.bottom > slot.top;
        }
        const auto state = slotValid ? widget_bridge::IntegrationState::Ready
                                     : widget_bridge::IntegrationState::Requested;
        SetSharedState(surface, state, slotValid ? &slot : nullptr);
        if (slotValid && (!lastReady || lastReadySessionId != lastSessionId || lastReadyGeneration != lastGeneration)) {
            AdapterTrace(L"ready-confirmed-valid-visible-slot", S_OK);
            lastReadySessionId = lastSessionId;
            lastReadyGeneration = lastGeneration;
            lastReady = true;
        } else if (!slotValid && lastReady) {
            AdapterTrace(L"ready-revoked-traditional-restored", S_FALSE);
            lastReady = false;
        }
    }
    if (mapping.surface) SetSharedState(mapping.surface, widget_bridge::IntegrationState::Traditional, nullptr);
    FreeLibraryAndExitThread(context->module, 0);
}

void ReapSurfaceReaders() noexcept {
    for (auto it = g_retiredReaders.begin(); it != g_retiredReaders.end();) {
        ReaderContext* context = *it;
        if (WaitForSingleObject(context->thread, 0) == WAIT_OBJECT_0) {
            CloseHandle(context->thread);
            CloseHandle(context->wake);
            CloseHandle(context->stop);
            delete context;
            it = g_retiredReaders.erase(it);
        } else ++it;
    }
}

void StartSurfaceReader(HWND taskbarWindow) noexcept {
    ReapSurfaceReaders();
    if (g_readerContext && g_readerContext->taskbar == taskbarWindow &&
        WaitForSingleObject(g_readerContext->thread, 0) == WAIT_TIMEOUT) {
        InterlockedExchange(&g_readerEnabled, 1);
        SetEvent(g_readerContext->wake);
        return;
    }
    StopSurfaceReader();
    InterlockedExchange(&g_readerEnabled, 1);
    InterlockedExchange(&g_uiSlotValid, 0);
    InterlockedExchange(&g_haveRenderedFrame, 0);
    auto* context = new (std::nothrow) ReaderContext{ taskbarWindow, nullptr, nullptr, nullptr, nullptr };
    if (!context) { InterlockedExchange(&g_readerEnabled, 0); return; }
    context->wake = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    context->stop = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (context->wake && context->stop &&
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                           reinterpret_cast<LPCWSTR>(&ReadSurfaceFrames), &context->module)) {
        context->thread = CreateThread(nullptr, 0, ReadSurfaceFrames, context, 0, nullptr);
        if (context->thread) g_readerContext = context;
        else FreeLibrary(context->module);
    }
    if (!context->thread) {
        if (context->wake) CloseHandle(context->wake);
        if (context->stop) CloseHandle(context->stop);
        delete context;
        InterlockedExchange(&g_readerEnabled, 0);
        return;
    }
    SetEvent(context->wake);
}

void StopSurfaceReader() noexcept {
    InterlockedExchange(&g_readerEnabled, 0);
    if (g_readerContext) {
        SetEvent(g_readerContext->stop);
        SetEvent(g_readerContext->wake);
        try { g_retiredReaders.push_back(g_readerContext); }
        catch (...) { /* retain the small context until Explorer exits */ }
        g_readerContext = nullptr;
    }
}

bool RefreshWidgetSurface(HWND taskbarWindow) noexcept {
    BatteryHubTaskbarXamlStatus status{};
    if (FAILED(BatteryHubTaskbarXamlGetStatus(&status)) || !status.attached || status.disabled ||
        InterlockedCompareExchange(&g_readerEnabled, 0, 0) == 0) {
        ResetWidgetSurface(true);
        return false;
    }
    if (InterlockedCompareExchange(&g_peerValid, 0, 0) == 0 ||
        InterlockedCompareExchange(&g_requestedByApp, 0, 0) == 0) return true;
    widget_bridge::SurfaceFrame frame;
    bool hasFrame = false;
    if (TryAcquireSRWLockExclusive(&g_pendingLock)) {
        if (g_pendingReady) {
            frame = std::move(g_pendingFrame);
            g_pendingReady = false;
            hasFrame = true;
        }
        ReleaseSRWLockExclusive(&g_pendingLock);
    }
    if (hasFrame) {
        HRESULT hr = BatteryHubTaskbarXamlSetInputTarget(frame.ownerWindow, batteryhub::xaml::kInputMessage);
        if (SUCCEEDED(hr))
            hr = BatteryHubTaskbarXamlUpdateSurface(frame.pixels.data(), frame.widthPx, frame.heightPx,
                                                    frame.dpi, frame.itemRects.data(),
                                                    frame.itemIds.data(), frame.itemAnchors.data(),
                                                    static_cast<std::uint32_t>(frame.itemRects.size()),
                                                    (frame.flags & 8u) ? TRUE : FALSE,
                                                    frame.sessionId, frame.generation);
        if (SUCCEEDED(hr))
            hr = BatteryHubTaskbarXamlSetWidgetLayout((frame.flags & 8u) ? TRUE : FALSE,
                                                      frame.itemIds.data(), frame.itemAnchors.data(),
                                                      static_cast<std::uint32_t>(frame.itemIds.size()),
                                                      frame.desiredGridColumn,
                                                      (frame.flags & 4u) ? TRUE : FALSE);
        if (FAILED(hr)) {
            AdapterTrace(L"writeablebitmap-update-failed", hr, &frame);
            InterlockedExchange(&g_uiSlotValid, 0);
            InterlockedExchange(&g_readerEnabled, 0);
            if (g_readerContext) SetEvent(g_readerContext->stop);
            BatteryHubTaskbarXamlDetach();
            return false;
        }
        InterlockedExchange(&g_haveRenderedFrame, 1);
        InterlockedExchange(&g_slotGeometryFailures, 0);
        AdapterTrace(L"writeablebitmap-updated", S_OK, &frame);
    }
    if (InterlockedCompareExchange(&g_haveRenderedFrame, 0, 0) == 0) return true;
    const HRESULT visibleHr = BatteryHubTaskbarXamlSetWidgetVisible(TRUE);
    if (FAILED(visibleHr)) {
        AdapterTrace(L"xaml-visible-request-failed", visibleHr);
        InterlockedExchange(&g_uiSlotValid, 0);
        BatteryHubTaskbarXamlDetach();
        return false;
    }
    RECT slot{};
    const HRESULT hr = BatteryHubTaskbarXamlGetWidgetScreenRect(taskbarWindow, &slot);
    if (FAILED(hr)) {
        if (hr == HRESULT_FROM_WIN32(ERROR_NOT_READY)) {
            // The XAML dispatcher has not presented this frame yet. Preserve the last committed
            // slot until the replacement geometry is ready, avoiding a fallback duplicate.
            AdapterTrace(L"xaml-render-pending", hr);
            return true;
        }
        InterlockedExchange(&g_uiSlotValid, 0);
        BatteryHubTaskbarXamlSetWidgetVisible(FALSE);
        AdapterTrace(L"xaml-geometry-invalid", hr);
        if (InterlockedIncrement(&g_slotGeometryFailures) >= 3) {
            InterlockedExchange(&g_readerEnabled, 0);
            if (g_readerContext) SetEvent(g_readerContext->stop);
            BatteryHubTaskbarXamlDetach();
        }
        return false;
    }
    AcquireSRWLockExclusive(&g_slotLock);
    g_uiSlot = slot;
    ReleaseSRWLockExclusive(&g_slotLock);
    InterlockedExchange(&g_uiSlotValid, 1);
    AdapterTrace(L"xaml-geometry-visible-valid", S_OK);
    if (g_readerContext) SetEvent(g_readerContext->wake);
    return true;
}
}

extern "C" __declspec(dllexport) LRESULT CALLBACK BatteryHubTaskbarHookProc(int code, WPARAM wParam, LPARAM lParam) {
    if (code == HC_ACTION) {
        auto const* message = reinterpret_cast<CWPSTRUCT const*>(lParam);
        const HWND taskbar = FindWindowW(kTrayWindowClass, nullptr);
        const bool onTaskbarThread = message && message->hwnd == taskbar &&
                                     GetWindowThreadProcessId(message->hwnd, nullptr) == GetCurrentThreadId();
        if (onTaskbarThread && message->message == SurfaceMessageId()) {
            if (g_readerContext) SetEvent(g_readerContext->wake); // background-only request
        } else if (onTaskbarThread && message->message == kDeliverFrameMessage) {
            BatteryHubTaskbarXamlStatus status{};
            if (SUCCEEDED(BatteryHubTaskbarXamlGetStatus(&status)) && status.attached && !status.disabled)
                RefreshWidgetSurface(taskbar);
            else
                ResetWidgetSurface(true);
        } else if (onTaskbarThread && message->message == kDetachSurfaceMessage) {
            ResetWidgetSurface(true);
        } else if (onTaskbarThread && message->message == WM_TIMER && message->wParam == kSurfaceHeartbeatTimer) {
            BatteryHubTaskbarXamlStatus status{};
            if (SUCCEEDED(BatteryHubTaskbarXamlGetStatus(&status)) && status.attached && !status.disabled)
                RefreshWidgetSurface(taskbar);
            else
                ResetWidgetSurface(true);
            ReapSurfaceReaders();
        } else if (message && (message->message == MessageId() || message->message == WM_TIMER)) {
            SharedView view;
            if (OpenShared(view)) {
                auto* state = view.state;
                if (GetCurrentThreadId() == state->targetThreadId && message->hwnd == state->taskbarWindow) {
                    if (message->message == MessageId()) {
                        LONG const current = InterlockedCompareExchange(&state->state, HostAttachRunning, HostAttachRequested);
                        if (current == HostAttachRequested) {
                            state->attachResult = Attach(state);
                            InterlockedExchange(&state->state, SUCCEEDED(state->attachResult) ? HostAttached : HostFailed);
                            Log(state, SUCCEEDED(state->attachResult) ? L"attach-complete" : L"attach-failed", state->attachResult);
                        } else if (current == HostDetachRequested) {
                            InterlockedExchange(&state->state, HostDetachRunning);
                            state->detachResult = Detach(state);
                            InterlockedExchange(&state->state, HostDetached);
                            Log(state, L"manual-detach-complete", state->detachResult);
                        }
                    } else if (state->timerId && message->wParam == state->timerId &&
                               InterlockedCompareExchange(&state->state, HostDetachRunning, HostAttached) == HostAttached) {
                        state->detachResult = Detach(state);
                        InterlockedExchange(&state->state, HostDetached);
                        Log(state, L"watchdog-detach-complete", state->detachResult);
                    }
                }
            }
        }
    }
    return CallNextHookEx(nullptr, code, wParam, lParam);
}

extern "C" __declspec(dllexport) LRESULT CALLBACK BatteryHubTaskbarGetMessageHookProc(int code, WPARAM wParam, LPARAM lParam) {
    if (code == HC_ACTION && wParam == PM_REMOVE && lParam) {
        auto const* message = reinterpret_cast<MSG const*>(lParam);
        const HWND taskbar = FindWindowW(kTrayWindowClass, nullptr);
        const bool onTaskbarThread = message->hwnd == taskbar && taskbar && IsWindow(taskbar) &&
                                     GetWindowThreadProcessId(taskbar, nullptr) == GetCurrentThreadId();
        if (onTaskbarThread && message->message == SurfaceMessageId()) {
            AdapterTrace(L"publisher-changed-message-received");
            if (g_readerContext) SetEvent(g_readerContext->wake);
        } else if (onTaskbarThread && message->message == kDeliverFrameMessage) {
            AdapterTrace(L"posted-frame-delivery-dispatched");
            BatteryHubTaskbarXamlStatus status{};
            if (SUCCEEDED(BatteryHubTaskbarXamlGetStatus(&status)) && status.attached && !status.disabled)
                RefreshWidgetSurface(taskbar);
            else
                ResetWidgetSurface(true);
        } else if (onTaskbarThread && message->message == kDetachSurfaceMessage) {
            AdapterTrace(L"posted-detach-dispatched");
            ResetWidgetSurface(true);
        } else if (onTaskbarThread && message->message == WM_TIMER && message->wParam == kSurfaceHeartbeatTimer) {
            AdapterTrace(L"surface-heartbeat-dispatched");
            BatteryHubTaskbarXamlStatus status{};
            if (SUCCEEDED(BatteryHubTaskbarXamlGetStatus(&status)) && status.attached && !status.disabled)
                RefreshWidgetSurface(taskbar);
            else
                ResetWidgetSurface(true);
            ReapSurfaceReaders();
        } else if (onTaskbarThread && (message->message == MessageId() || message->message == WM_TIMER)) {
            SharedView view;
            if (OpenShared(view)) {
                auto* state = view.state;
                if (GetCurrentThreadId() == state->targetThreadId && message->hwnd == state->taskbarWindow) {
                    if (message->message == MessageId()) {
                        LONG const current = InterlockedCompareExchange(&state->state, HostAttachRunning, HostAttachRequested);
                        if (current == HostAttachRequested) {
                            state->attachResult = Attach(state);
                            InterlockedExchange(&state->state, SUCCEEDED(state->attachResult) ? HostAttached : HostFailed);
                            Log(state, SUCCEEDED(state->attachResult) ? L"attach-complete" : L"attach-failed", state->attachResult);
                        } else if (current == HostDetachRequested) {
                            InterlockedExchange(&state->state, HostDetachRunning);
                            state->detachResult = Detach(state);
                            InterlockedExchange(&state->state, HostDetached);
                            Log(state, L"manual-detach-complete", state->detachResult);
                        }
                    } else if (state->timerId && message->wParam == state->timerId &&
                               InterlockedCompareExchange(&state->state, HostDetachRunning, HostAttached) == HostAttached) {
                        state->detachResult = Detach(state);
                        InterlockedExchange(&state->state, HostDetached);
                        Log(state, L"watchdog-detach-complete", state->detachResult);
                    }
                }
            }
        }
    }
    return CallNextHookEx(nullptr, code, wParam, lParam);
}
