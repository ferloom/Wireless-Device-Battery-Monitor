#pragma once

#include <windows.h>

namespace batteryhub::win32 {

// GetWindowThreadProcessId returns the TID; the PID is written through its output parameter.
constexpr DWORD ProcessIdFromWindowLookup(DWORD threadId, DWORD processId) noexcept {
    return threadId ? processId : 0;
}

inline DWORD ProcessIdForWindow(HWND window) noexcept {
    if (!window) return 0;
    DWORD processId = 0;
    const DWORD threadId = ::GetWindowThreadProcessId(window, &processId);
    return ProcessIdFromWindowLookup(threadId, processId);
}

} // namespace batteryhub::win32
