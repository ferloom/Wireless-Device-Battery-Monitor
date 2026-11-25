#pragma once

#include <windows.h>
#include <string>

#include "widget_bridge_protocol.h"

namespace widget_bridge {

// BatteryHub is the sole writer of rendered frames. The mapping is local to the logon
// session and has a DACL for the current user; Start() refuses non-Explorer targets.
class Publisher {
public:
    Publisher() = default;
    Publisher(Publisher const&) = delete;
    Publisher& operator=(Publisher const&) = delete;
    ~Publisher();

    bool Start(HWND ownerWindow, HWND taskbarWindow) noexcept;
    void Stop() noexcept;
    bool SetRequested(bool requested) noexcept;
    bool Publish(DWORD const* bgra, std::uint32_t width, std::uint32_t height,
                 std::uint32_t dpi, Rect const* itemRects, std::uint64_t const* itemIds,
                 std::int32_t const* itemAnchors, std::uint32_t itemCount, bool individual,
                 bool lightTheme, bool hover, bool positionLocked, std::int32_t desiredGridColumn) noexcept;
    bool ReadReadySlot(RECT& screenRect) const noexcept;
    IntegrationState State() const noexcept;
    bool IsStarted() const noexcept { return m_surface != nullptr; }

private:
    void Notify() noexcept;
    HANDLE m_mapping = nullptr;
    SharedSurface* m_surface = nullptr;
    HWND m_ownerWindow = nullptr;
    HWND m_taskbarWindow = nullptr;
    UINT m_changedMessage = 0;
    std::uint64_t m_sessionId = 0;
    std::uint32_t m_generation = 0;
    bool m_requested = false;
    mutable bool m_lastReady = false;
    mutable RECT m_lastReadySlot{};
};

} // namespace widget_bridge
