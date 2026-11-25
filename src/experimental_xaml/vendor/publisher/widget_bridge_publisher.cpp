#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <sddl.h>

#include "widget_bridge_publisher.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <vector>

namespace widget_bridge {
namespace {

bool ProcessImage(DWORD pid, std::wstring& path, HANDLE& process) noexcept {
    process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
    if (!process) return false;
    wchar_t buffer[32768]{};
    DWORD chars = ARRAYSIZE(buffer);
    if (!QueryFullProcessImageNameW(process, 0, buffer, &chars)) {
        CloseHandle(process);
        process = nullptr;
        return false;
    }
    path.assign(buffer, chars);
    return true;
}

bool ImageIs(std::wstring const& path, wchar_t const* expected) noexcept {
    const auto slash = path.find_last_of(L"\\/");
    const wchar_t* base = path.c_str() + (slash == std::wstring::npos ? 0 : slash + 1);
    return _wcsicmp(base, expected) == 0;
}

bool GetTokenUserBytes(HANDLE process, std::vector<BYTE>& storage) noexcept {
    HANDLE token = nullptr;
    if (!OpenProcessToken(process, TOKEN_QUERY, &token)) return false;
    DWORD bytes = 0;
    GetTokenInformation(token, TokenUser, nullptr, 0, &bytes);
    if (!bytes) { CloseHandle(token); return false; }
    try { storage.resize(bytes); }
    catch (...) { CloseHandle(token); return false; }
    const bool ok = GetTokenInformation(token, TokenUser, storage.data(), bytes, &bytes) != FALSE;
    CloseHandle(token);
    return ok;
}

bool SameUser(HANDLE a, HANDLE b) noexcept {
    std::vector<BYTE> first, second;
    if (!GetTokenUserBytes(a, first) || !GetTokenUserBytes(b, second)) return false;
    auto* userA = reinterpret_cast<TOKEN_USER*>(first.data());
    auto* userB = reinterpret_cast<TOKEN_USER*>(second.data());
    return EqualSid(userA->User.Sid, userB->User.Sid) != FALSE;
}

bool CurrentUserSecurity(SECURITY_DESCRIPTOR*& descriptor, SECURITY_ATTRIBUTES& attributes) noexcept {
    HANDLE token = nullptr;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) return false;
    DWORD bytes = 0;
    GetTokenInformation(token, TokenUser, nullptr, 0, &bytes);
    if (!bytes) { CloseHandle(token); return false; }
    std::vector<BYTE> storage;
    try { storage.resize(bytes); }
    catch (...) { CloseHandle(token); return false; }
    if (!GetTokenInformation(token, TokenUser, storage.data(), bytes, &bytes)) {
        CloseHandle(token);
        return false;
    }
    CloseHandle(token);
    LPWSTR sidText = nullptr;
    if (!ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(storage.data())->User.Sid, &sidText)) return false;
    std::wstring sddl = L"D:P(A;;GA;;;";
    sddl += sidText;
    sddl += L")";
    LocalFree(sidText);
    return ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.c_str(), SDDL_REVISION_1,
                                                                  reinterpret_cast<PSECURITY_DESCRIPTOR*>(&descriptor),
                                                                  nullptr) != FALSE &&
           (attributes = { sizeof(attributes), descriptor, FALSE }, true);
}

bool SameLogon(HANDLE owner, DWORD ownerPid, HANDLE target, DWORD targetPid) noexcept {
    DWORD ownerSession = 0, targetSession = 0;
    return ProcessIdToSessionId(ownerPid, &ownerSession) && ProcessIdToSessionId(targetPid, &targetSession) &&
           ownerSession == targetSession && SameUser(owner, target);
}

std::wstring g_tracePath;
SRWLOCK g_traceLock = SRWLOCK_INIT;

void Trace(wchar_t const* stage, std::uint64_t sessionId, std::uint32_t generation,
           std::uint32_t width, std::uint32_t height, std::uint32_t dpi,
           std::uint32_t byteCount, std::uint64_t hash, bool alphaValid) noexcept {
    if (g_tracePath.empty()) return;
    SYSTEMTIME now{};
    GetSystemTime(&now);
    wchar_t line[768]{};
    const int chars = swprintf_s(line,
        L"{\"tsUtc\":\"%04u-%02u-%02uT%02u:%02u:%02u.%03uZ\",\"pid\":%lu,\"tid\":%lu,"
        L"\"session\":\"%016llX\",\"frame\":%lu,\"stage\":\"%s\",\"widthPx\":%lu,"
        L"\"heightPx\":%lu,\"dpi\":%lu,\"stride\":%lu,\"pixelBytes\":%lu,"
        L"\"format\":\"BGRA32-premultiplied\",\"alphaValid\":%s,\"pixelHash\":\"%016llX\"}\r\n",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds,
        GetCurrentProcessId(), GetCurrentThreadId(), static_cast<unsigned long long>(sessionId), generation,
        stage, width, height, dpi, width * 4u, byteCount, alphaValid ? L"true" : L"false",
        static_cast<unsigned long long>(hash));
    if (chars <= 0 || static_cast<std::size_t>(chars) >= ARRAYSIZE(line)) return;
    char utf8[sizeof(line) * 4]{};
    const int bytes = WideCharToMultiByte(CP_UTF8, 0, line, chars, utf8, static_cast<int>(sizeof(utf8)), nullptr, nullptr);
    if (bytes <= 0) return;
    AcquireSRWLockExclusive(&g_traceLock);
    HANDLE file = CreateFileW(g_tracePath.c_str(), FILE_APPEND_DATA,
                              FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                              nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file != INVALID_HANDLE_VALUE) {
        DWORD written = 0;
        WriteFile(file, utf8, static_cast<DWORD>(bytes), &written, nullptr);
        CloseHandle(file);
    }
    ReleaseSRWLockExclusive(&g_traceLock);
}

std::uint64_t NewSessionId() noexcept {
    FILETIME ft{};
    LARGE_INTEGER counter{};
    GetSystemTimeAsFileTime(&ft);
    QueryPerformanceCounter(&counter);
    const std::uint64_t fileTime = (static_cast<std::uint64_t>(ft.dwHighDateTime) << 32) | ft.dwLowDateTime;
    std::uint64_t id = fileTime ^ static_cast<std::uint64_t>(counter.QuadPart) ^
                       (static_cast<std::uint64_t>(GetCurrentProcessId()) << 32);
    return id ? id : 1;
}

} // namespace

Publisher::~Publisher() { Stop(); }

bool Publisher::Start(HWND ownerWindow, HWND taskbarWindow) noexcept {
    if (m_surface) return ownerWindow == m_ownerWindow && taskbarWindow == m_taskbarWindow;
    if (!ownerWindow || !taskbarWindow || !IsWindow(ownerWindow) || !IsWindow(taskbarWindow)) return false;
    wchar_t className[64]{};
    if (!GetClassNameW(taskbarWindow, className, ARRAYSIZE(className)) || _wcsicmp(className, L"Shell_TrayWnd"))
        return false;
    DWORD ownerPid = 0, targetPid = 0;
    if (!GetWindowThreadProcessId(ownerWindow, &ownerPid) || ownerPid != GetCurrentProcessId() ||
        !GetWindowThreadProcessId(taskbarWindow, &targetPid) || !targetPid || targetPid == ownerPid) return false;

    std::wstring ownerPath, targetPath;
    HANDLE ownerProcess = nullptr, targetProcess = nullptr;
    if (!ProcessImage(ownerPid, ownerPath, ownerProcess) || !ProcessImage(targetPid, targetPath, targetProcess)) {
        if (ownerProcess) CloseHandle(ownerProcess);
        if (targetProcess) CloseHandle(targetProcess);
        return false;
    }
    bool identityOk = ImageIs(ownerPath, L"BatteryHub-Experimental.exe") && ImageIs(targetPath, L"explorer.exe") &&
                      SameLogon(ownerProcess, ownerPid, targetProcess, targetPid);
    DWORD ownerSession = 0, targetSession = 0;
    identityOk = identityOk && ProcessIdToSessionId(ownerPid, &ownerSession) &&
                 ProcessIdToSessionId(targetPid, &targetSession);
    CloseHandle(ownerProcess);
    CloseHandle(targetProcess);
    if (!identityOk) return false;

    wchar_t name[128]{};
    if (swprintf_s(name, L"%s%lu", kMappingPrefix, targetPid) < 0) return false;
    SECURITY_DESCRIPTOR* descriptor = nullptr;
    SECURITY_ATTRIBUTES attributes{};
    if (!CurrentUserSecurity(descriptor, attributes)) return false;
    bool reusedStaleMapping = false;
    DWORD staleOwnerPid = 0;
    SetLastError(ERROR_SUCCESS);
    m_mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, &attributes, PAGE_READWRITE,
                                   static_cast<DWORD>(kMappingBytes >> 32),
                                   static_cast<DWORD>(kMappingBytes & 0xffffffffu), name);
    const bool mappingExists = m_mapping && GetLastError() == ERROR_ALREADY_EXISTS;
    LocalFree(descriptor);
    if (!m_mapping) return false;
    if (mappingExists) {
        auto* previous = static_cast<SharedSurface*>(MapViewOfFile(m_mapping, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, kMappingBytes));
        if (!previous) { CloseHandle(m_mapping); m_mapping = nullptr; return false; }
        const DWORD previousOwner = previous->ownerPid;
        const LONG requested = InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(&previous->requested), 0, 0);
        const LONG state = InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(&previous->integrationState), 0, 0);
        const LONG sequence = InterlockedCompareExchange(reinterpret_cast<volatile LONG*>(&previous->snapshotSequence), 0, 0);
        bool ownerExited = false;
        if (previousOwner && previousOwner != ownerPid) {
            HANDLE oldProcess = OpenProcess(SYNCHRONIZE, FALSE, previousOwner);
            if (oldProcess) {
                ownerExited = WaitForSingleObject(oldProcess, 0) == WAIT_OBJECT_0;
                CloseHandle(oldProcess);
            } else {
                ownerExited = GetLastError() == ERROR_INVALID_PARAMETER;
            }
        }
        const bool inactiveState = state >= static_cast<LONG>(IntegrationState::Traditional) &&
            state <= static_cast<LONG>(IntegrationState::Failed) &&
            CanReuseInactiveMapping(ownerExited, requested != 0, static_cast<IntegrationState>(state));
        const bool stale = previous->magic == kMagic && previous->version == kVersion &&
            previous->targetPid == targetPid && previous->ownerSession == ownerSession &&
            previous->targetSession == targetSession && inactiveState && !(sequence & 1);
        if (!stale) {
            UnmapViewOfFile(previous);
            CloseHandle(m_mapping);
            m_mapping = nullptr;
            return false;
        }
        InterlockedExchange(reinterpret_cast<volatile LONG*>(&previous->snapshotSequence), sequence + 1);
        MemoryBarrier();
        ZeroMemory(reinterpret_cast<BYTE*>(previous) + offsetof(SharedSurface, ownerPid),
                   sizeof(SharedSurface) - offsetof(SharedSurface, ownerPid));
        MemoryBarrier();
        InterlockedExchange(reinterpret_cast<volatile LONG*>(&previous->snapshotSequence), sequence + 2);
        UnmapViewOfFile(previous);
        reusedStaleMapping = true;
        staleOwnerPid = previousOwner;
    }
    m_surface = static_cast<SharedSurface*>(MapViewOfFile(m_mapping, FILE_MAP_READ | FILE_MAP_WRITE,
                                                          0, 0, kMappingBytes));
    if (!m_surface) {
        CloseHandle(m_mapping);
        m_mapping = nullptr;
        return false;
    }
    m_ownerWindow = ownerWindow;
    m_taskbarWindow = taskbarWindow;
    m_changedMessage = RegisterWindowMessageW(kSurfaceChangedMessage);
    if (!m_changedMessage) { Stop(); return false; }

    if (g_tracePath.empty()) {
        wchar_t tracePath[32768]{};
        const DWORD chars = GetEnvironmentVariableW(L"BATTERYHUB_XAML_TRACE_PATH", tracePath, ARRAYSIZE(tracePath));
        if (chars && chars < ARRAYSIZE(tracePath)) {
            try { g_tracePath.assign(tracePath, chars); } catch (...) { g_tracePath.clear(); }
        }
    }

    if (reusedStaleMapping)
        Trace(L"stale-publisher-mapping-reused", 0, 0, 0, 0, 0, 0, staleOwnerPid, true);

    m_surface->magic = kMagic;
    m_surface->version = kVersion;
    m_sessionId = NewSessionId();
    m_surface->sessionId = m_sessionId;
    m_surface->ownerPid = ownerPid;
    m_surface->targetPid = targetPid;
    m_surface->ownerSession = ownerSession;
    m_surface->targetSession = targetSession;
    m_surface->ownerWindow = reinterpret_cast<std::uintptr_t>(ownerWindow);
    m_surface->taskbarWindow = reinterpret_cast<std::uintptr_t>(taskbarWindow);
    m_surface->integrationState = static_cast<std::int32_t>(IntegrationState::Traditional);
    m_requested = false;
    Trace(L"publisher-start", m_sessionId, 0, 0, 0, 0, 0, 0, true);
    return true;
}

void Publisher::Stop() noexcept {
    if (m_surface) Trace(L"publisher-stop", m_sessionId, m_generation, m_surface->widthPx,
                         m_surface->heightPx, m_surface->dpi, m_surface->pixelBytes, 0, true);
    if (m_surface && m_requested) SetRequested(false);
    if (m_surface) UnmapViewOfFile(m_surface);
    if (m_mapping) CloseHandle(m_mapping);
    m_surface = nullptr;
    m_mapping = nullptr;
    m_ownerWindow = m_taskbarWindow = nullptr;
    m_changedMessage = 0;
    m_requested = false;
    m_generation = 0;
    m_sessionId = 0;
    m_lastReady = false;
    m_lastReadySlot = {};
}

void Publisher::Notify() noexcept {
    if (m_taskbarWindow && IsWindow(m_taskbarWindow) && m_changedMessage)
        PostMessageW(m_taskbarWindow, m_changedMessage, 0, 0);
}

bool Publisher::SetRequested(bool requested) noexcept {
    if (!m_surface) return false;
    if (m_requested == requested) return true;
    m_requested = requested;
    if (!requested && m_lastReady)
        Trace(L"ready-revoked-traditional-restored", m_sessionId, m_generation, 0, 0, 0, 0, 0, true);
    if (!requested) {
        m_lastReady = false;
        m_lastReadySlot = {};
    }
    InterlockedExchange(reinterpret_cast<volatile LONG*>(&m_surface->requested), requested ? 1 : 0);
    Trace(requested ? L"publisher-requested" : L"publisher-unrequested", m_sessionId,
          m_generation, m_surface->widthPx, m_surface->heightPx, m_surface->dpi,
          m_surface->pixelBytes, 0, true);
    Notify();
    return true;
}

bool Publisher::Publish(DWORD const* bgra, std::uint32_t width, std::uint32_t height,
                        std::uint32_t dpi, Rect const* itemRects, std::uint64_t const* itemIds,
                        std::int32_t const* itemAnchors, std::uint32_t itemCount, bool individual,
                        bool lightTheme, bool hover, bool positionLocked,
                        std::int32_t desiredGridColumn) noexcept {
    if (!m_surface || !m_requested || !bgra || !ValidSurface(width, height,
            static_cast<std::uint32_t>(static_cast<std::uint64_t>(width) * height * 4u), itemCount, dpi) ||
        (itemCount && (!itemRects || !itemIds || !itemAnchors))) return false;
    for (std::uint32_t i = 0; i < itemCount; ++i) {
        if (!ValidRect(itemRects[i], width, height) || itemAnchors[i] < kMinGridAnchor ||
            itemAnchors[i] >= static_cast<std::int32_t>(kMaxGridColumns)) return false;
        if (itemIds[i]) for (std::uint32_t j = 0; j < i; ++j)
            if (itemIds[j] == itemIds[i]) return false;
    }
    const auto pixels = static_cast<std::size_t>(width) * height;

    SharedSurface& s = *m_surface;
    volatile LONG* sequence = reinterpret_cast<volatile LONG*>(&s.snapshotSequence);
    if (InterlockedCompareExchange(sequence, 0, 0) & 1) return false;
    if (desiredGridColumn < kMinGridAnchor || desiredGridColumn >= static_cast<std::int32_t>(kMaxGridColumns)) return false;
    const std::uint32_t flags = (lightTheme ? 1u : 0u) | (hover ? 2u : 0u) |
                                (positionLocked ? 4u : 0u) | (individual ? 8u : 0u);
    const std::uint32_t bytes = static_cast<std::uint32_t>(pixels * sizeof(std::uint32_t));
    const bool same = s.widthPx == width && s.heightPx == height && s.dpi == dpi &&
                      s.itemCount == itemCount && s.pixelBytes == bytes && s.flags == flags &&
                      s.desiredGridColumn == desiredGridColumn &&
                      (!itemCount || std::memcmp(s.itemRects, itemRects, itemCount * sizeof(Rect)) == 0) &&
                      (!itemCount || std::memcmp(s.itemIds, itemIds, itemCount * sizeof(std::uint64_t)) == 0) &&
                      (!itemCount || std::memcmp(s.itemAnchors, itemAnchors, itemCount * sizeof(std::int32_t)) == 0) &&
                      std::memcmp(s.pixels, bgra, bytes) == 0;
    if (same) return true;
    const bool alphaValid = ValidPremultipliedBgra(reinterpret_cast<std::uint32_t const*>(bgra), pixels);
    if (!alphaValid) {
        Trace(L"publisher-rejected-alpha", m_sessionId, m_generation, width, height, dpi,
              bytes, PixelHash(reinterpret_cast<std::uint32_t const*>(bgra), pixels), false);
        return false;
    }

    InterlockedIncrement(sequence); // odd while BatteryHub writes a frame
    s.dpi = dpi;
    s.widthPx = width;
    s.heightPx = height;
    s.itemCount = itemCount;
    s.pixelBytes = bytes;
    s.flags = flags;
    s.desiredGridColumn = desiredGridColumn;
    if (itemCount) {
        std::memcpy(s.itemRects, itemRects, itemCount * sizeof(Rect));
        std::memcpy(s.itemIds, itemIds, itemCount * sizeof(std::uint64_t));
        std::memcpy(s.itemAnchors, itemAnchors, itemCount * sizeof(std::int32_t));
    }
    std::memcpy(s.pixels, bgra, bytes);
    s.generation = ++m_generation;
    MemoryBarrier();
    InterlockedIncrement(sequence); // even: the frame is complete
    Trace(L"frame-published", m_sessionId, m_generation, width, height, dpi, bytes,
          PixelHash(reinterpret_cast<std::uint32_t const*>(bgra), pixels), alphaValid);
    Notify();
    return true;
}

IntegrationState Publisher::State() const noexcept {
    if (!m_surface) return IntegrationState::Traditional;
    const LONG value = InterlockedCompareExchange(
        reinterpret_cast<volatile LONG*>(&m_surface->integrationState), 0, 0);
    return value >= static_cast<LONG>(IntegrationState::Traditional) &&
                   value <= static_cast<LONG>(IntegrationState::Failed)
               ? static_cast<IntegrationState>(value) : IntegrationState::Failed;
}

bool Publisher::ReadReadySlot(RECT& screenRect) const noexcept {
    auto revoke = [&]() noexcept {
        if (m_lastReady && m_surface)
            Trace(L"ready-revoked-traditional-restored", m_sessionId, m_generation,
                  0, 0, 0, 0, 0, true);
        m_lastReady = false;
        m_lastReadySlot = {};
        return false;
    };
    if (!m_surface || !m_requested) return revoke();
    const LONG64 now = static_cast<LONG64>(GetTickCount64());
    const LONG64 beat = InterlockedCompareExchange64(
        reinterpret_cast<volatile LONG64*>(&m_surface->integrationHeartbeatMs), 0, 0);
    if (beat <= 0 || now < beat || now - beat > 3000) return revoke();
    auto keepPrevious = [&]() noexcept {
        const auto state = State();
        const bool requested = InterlockedCompareExchange(
            reinterpret_cast<volatile LONG*>(&m_surface->requested), 0, 0) != 0;
        const bool fresh = beat > 0 && now >= beat && now - beat <= 3000;
        const bool rectValid = m_lastReadySlot.right > m_lastReadySlot.left &&
                               m_lastReadySlot.bottom > m_lastReadySlot.top;
        if (!CanKeepPreviousReadySlot(m_lastReady, state, requested, fresh) || !rectValid)
            return revoke();
        screenRect = m_lastReadySlot;
        return true;
    };
    auto* frameSequence = reinterpret_cast<volatile LONG*>(const_cast<std::int32_t*>(&m_surface->snapshotSequence));
    const LONG frameBefore = InterlockedCompareExchange(frameSequence, 0, 0);
    if (frameBefore < 0 || (frameBefore & 1)) return keepPrevious();
    const auto sessionId = m_surface->sessionId;
    const auto generation = m_surface->generation;
    const auto width = m_surface->widthPx;
    const auto height = m_surface->heightPx;
    const auto dpi = m_surface->dpi;
    const auto itemCount = m_surface->itemCount;
    const auto pixelBytes = m_surface->pixelBytes;
    const auto flags = m_surface->flags;
    const bool validContent = m_surface->magic == kMagic && m_surface->version == kVersion && sessionId &&
        !(flags & ~15u) && ValidSurface(width, height, pixelBytes, itemCount, dpi) && ValidFrame(*m_surface);
    MemoryBarrier();
    if (frameBefore != InterlockedCompareExchange(frameSequence, 0, 0)) return keepPrevious();
    if (!validContent) return revoke();

    auto* sequence = reinterpret_cast<volatile LONG*>(const_cast<std::int32_t*>(&m_surface->slotSequence));
    const LONG before = InterlockedCompareExchange(sequence, 0, 0);
    if (before < 0 || (before & 1)) return keepPrevious();
    const Rect rect = m_surface->slotScreenRect;
    MemoryBarrier();
    const bool slotValid = ValidSlot(rect, *m_surface);
    const auto state = State();
    const bool requested = InterlockedCompareExchange(
        reinterpret_cast<volatile LONG*>(&m_surface->requested), 0, 0) != 0;
    const bool fresh = beat > 0 && now >= beat && now - beat <= 3000;
    if (before != InterlockedCompareExchange(sequence, 0, 0)) return keepPrevious();
    if (!ShouldHideTraditional(state, requested, validContent, slotValid, fresh)) {
        if (!slotValid && CanKeepPreviousReadySlot(m_lastReady, state, requested, fresh) &&
            m_lastReadySlot.right > m_lastReadySlot.left && m_lastReadySlot.bottom > m_lastReadySlot.top) {
            screenRect = m_lastReadySlot;
            return true;
        }
        return revoke();
    }
    screenRect = { rect.left, rect.top, rect.right, rect.bottom };
    if (!m_lastReady)
        Trace(L"ready-confirmed-widget-hidden", sessionId, generation, width, height, dpi, pixelBytes, 0, true);
    m_lastReady = true;
    m_lastReadySlot = screenRect;
    return true;
}

} // namespace widget_bridge
