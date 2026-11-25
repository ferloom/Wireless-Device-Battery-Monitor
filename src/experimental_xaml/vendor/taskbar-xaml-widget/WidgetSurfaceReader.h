#pragma once

#include <windows.h>
#include "../publisher/widget_bridge_protocol.h"

#include <cstring>
#include <string>
#include <utility>
#include <vector>

namespace widget_bridge {

struct SurfaceFrame {
    std::uint64_t sessionId = 0;
    std::uint32_t generation = 0, dpi = 96, widthPx = 0, heightPx = 0, flags = 0;
    std::int32_t desiredGridColumn = -1;
    HWND ownerWindow = nullptr;
    std::vector<Rect> itemRects;
    std::vector<std::uint64_t> itemIds;
    std::vector<std::int32_t> itemAnchors;
    std::vector<std::uint32_t> pixels;
};

inline bool ValidatePeerIdentity(SharedSurface const& surface, DWORD targetPid,
                                 HWND taskbar, bool requireCurrentTarget = true) noexcept {
    if (!taskbar || !IsWindow(taskbar) || surface.targetPid != targetPid ||
        (requireCurrentTarget && targetPid != GetCurrentProcessId()) || surface.ownerPid == targetPid ||
        surface.ownerSession != surface.targetSession) return false;
    wchar_t className[64]{};
    if (!GetClassNameW(taskbar, className, ARRAYSIZE(className)) || _wcsicmp(className, L"Shell_TrayWnd"))
        return false;
    DWORD windowPid = 0;
    if (GetWindowThreadProcessId(taskbar, &windowPid) == 0 || windowPid != targetPid) return false;

    DWORD ownerSession = 0, targetSession = 0;
    if (!ProcessIdToSessionId(surface.ownerPid, &ownerSession) ||
        !ProcessIdToSessionId(targetPid, &targetSession) || ownerSession != targetSession ||
        ownerSession != surface.ownerSession) return false;
    HANDLE owner = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, surface.ownerPid);
    HANDLE target = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, targetPid);
    if (!owner || !target) {
        if (owner) CloseHandle(owner);
        if (target) CloseHandle(target);
        return false;
    }
    auto sameUser = [](HANDLE a, HANDLE b) {
        auto readUser = [](HANDLE process, std::vector<BYTE>& bytes) {
            HANDLE token = nullptr;
            if (!OpenProcessToken(process, TOKEN_QUERY, &token)) return false;
            DWORD size = 0;
            GetTokenInformation(token, TokenUser, nullptr, 0, &size);
            if (!size) { CloseHandle(token); return false; }
            try { bytes.resize(size); } catch (...) { CloseHandle(token); return false; }
            const bool ok = GetTokenInformation(token, TokenUser, bytes.data(), size, &size) != FALSE;
            CloseHandle(token);
            return ok;
        };
        std::vector<BYTE> ua, ub;
        if (!readUser(a, ua) || !readUser(b, ub)) return false;
        return EqualSid(reinterpret_cast<TOKEN_USER*>(ua.data())->User.Sid,
                        reinterpret_cast<TOKEN_USER*>(ub.data())->User.Sid) != FALSE;
    };
    bool ok = sameUser(owner, target);
    if (!requireCurrentTarget) {
        DWORD consumerSession = 0;
        ok = ok && ProcessIdToSessionId(GetCurrentProcessId(), &consumerSession) &&
             consumerSession == targetSession && sameUser(GetCurrentProcess(), target);
    }
    wchar_t ownerPath[32768]{}, targetPath[32768]{};
    DWORD ownerChars = ARRAYSIZE(ownerPath), targetChars = ARRAYSIZE(targetPath);
    ok = ok && QueryFullProcessImageNameW(owner, 0, ownerPath, &ownerChars) &&
         QueryFullProcessImageNameW(target, 0, targetPath, &targetChars);
    CloseHandle(owner);
    CloseHandle(target);
    auto isImage = [](wchar_t const* path, wchar_t const* expected) {
        const wchar_t* base = wcsrchr(path, L'\\');
        if (!base) base = wcsrchr(path, L'/');
        return _wcsicmp(base ? base + 1 : path, expected) == 0;
    };
    return ok && isImage(ownerPath, L"BatteryHub-Experimental.exe") && isImage(targetPath, L"explorer.exe");
}

// Caller maps the versioned Local\ section and invokes this on the taskbar's XAML thread.
// A sequence change discards the whole copy; the next posted frame notification can retry it.
inline bool ReadStableFrame(SharedSurface const* source, DWORD targetPid,
                            HWND taskbar, SurfaceFrame& frame) noexcept {
    if (!source || !taskbar || !IsWindow(taskbar)) return false;
    auto* sequence = const_cast<volatile LONG*>(reinterpret_cast<volatile LONG const*>(&source->snapshotSequence));
    const LONG before = InterlockedCompareExchange(sequence, 0, 0);
    if (before < 0 || (before & 1) || source->magic != kMagic || source->version != kVersion ||
        source->targetPid != targetPid || !source->ownerPid ||
        source->taskbarWindow != reinterpret_cast<std::uintptr_t>(taskbar)) return false;
    DWORD ownerPid = 0;
    if (!source->ownerWindow || !IsWindow(reinterpret_cast<HWND>(source->ownerWindow)) ||
        !GetWindowThreadProcessId(reinterpret_cast<HWND>(source->ownerWindow), &ownerPid) ||
        ownerPid != source->ownerPid) return false;

    const auto width = source->widthPx, height = source->heightPx;
    const auto count = source->itemCount, bytes = source->pixelBytes, dpi = source->dpi;
    if (!ValidSurface(width, height, bytes, count, dpi)) return false;
    SurfaceFrame next;
    try {
        next.generation = source->generation;
        next.sessionId = source->sessionId;
        next.dpi = dpi;
        next.widthPx = width;
        next.heightPx = height;
        next.flags = source->flags;
        next.desiredGridColumn = source->desiredGridColumn;
        next.ownerWindow = reinterpret_cast<HWND>(source->ownerWindow);
        next.itemRects.assign(source->itemRects, source->itemRects + count);
        next.itemIds.assign(source->itemIds, source->itemIds + count);
        next.itemAnchors.assign(source->itemAnchors, source->itemAnchors + count);
        for (std::size_t i = 0; i < next.itemRects.size(); ++i) {
            if (!ValidRect(next.itemRects[i], width, height) ||
                next.itemAnchors[i] < kMinGridAnchor ||
                next.itemAnchors[i] >= static_cast<std::int32_t>(kMaxGridColumns)) return false;
            if (next.itemIds[i]) for (std::size_t j = 0; j < i; ++j)
                if (next.itemIds[j] == next.itemIds[i]) return false;
        }
        next.pixels.resize(bytes / sizeof(std::uint32_t));
        std::memcpy(next.pixels.data(), source->pixels, bytes);
    } catch (...) {
        return false;
    }
    MemoryBarrier();
    const LONG after = InterlockedCompareExchange(sequence, 0, 0);
    if (before != after || (after & 1) || !ValidPremultipliedBgra(next.pixels.data(), next.pixels.size())) return false;
    frame = std::move(next);
    return true;
}

inline bool ReadStableFrameIfChanged(SharedSurface const* source, DWORD targetPid, HWND taskbar,
                                     std::uint64_t previousSessionId, std::uint32_t previousGeneration,
                                     SurfaceFrame& frame,
                                     bool& changed) noexcept {
    changed = false;
    if (!source || !taskbar || !IsWindow(taskbar)) return false;
    auto* sequence = const_cast<volatile LONG*>(reinterpret_cast<volatile LONG const*>(&source->snapshotSequence));
    const LONG before = InterlockedCompareExchange(sequence, 0, 0);
    if (before < 0 || (before & 1) || source->targetPid != targetPid ||
        source->taskbarWindow != reinterpret_cast<std::uintptr_t>(taskbar)) return false;
    if (source->sessionId == previousSessionId && source->generation == previousGeneration) {
        if (!ValidFrame(*source)) return false;
        MemoryBarrier();
        if (before != InterlockedCompareExchange(sequence, 0, 0)) return false;
        DWORD ownerPid = 0;
        return source->ownerWindow && IsWindow(reinterpret_cast<HWND>(source->ownerWindow)) &&
               GetWindowThreadProcessId(reinterpret_cast<HWND>(source->ownerWindow), &ownerPid) &&
               ownerPid == source->ownerPid;
    }
    changed = ReadStableFrame(source, targetPid, taskbar, frame);
    return changed;
}

} // namespace widget_bridge
