#include "widget_bridge_publisher.h"

#include <cstdint>
#include <new>

extern "C" {

void* bhx_publisher_new() noexcept {
    return new (std::nothrow) widget_bridge::Publisher();
}

void bhx_publisher_delete(void* value) noexcept {
    delete static_cast<widget_bridge::Publisher*>(value);
}

std::int32_t bhx_publisher_start(void* value, std::uintptr_t owner, std::uintptr_t taskbar) noexcept {
    auto* publisher = static_cast<widget_bridge::Publisher*>(value);
    return publisher && publisher->Start(reinterpret_cast<HWND>(owner), reinterpret_cast<HWND>(taskbar));
}

std::int32_t bhx_publisher_restart(void* value, std::uintptr_t owner, std::uintptr_t taskbar) noexcept {
    auto* publisher = static_cast<widget_bridge::Publisher*>(value);
    if (!publisher) return 0;
    publisher->Stop();
    return publisher->Start(reinterpret_cast<HWND>(owner), reinterpret_cast<HWND>(taskbar));
}

void bhx_publisher_stop(void* value) noexcept {
    if (auto* publisher = static_cast<widget_bridge::Publisher*>(value)) publisher->Stop();
}

std::int32_t bhx_publisher_request(void* value, std::int32_t requested) noexcept {
    auto* publisher = static_cast<widget_bridge::Publisher*>(value);
    return publisher && publisher->SetRequested(requested != 0);
}

std::int32_t bhx_publisher_publish(void* value, std::uint32_t const* pixels,
                                  std::uint32_t width, std::uint32_t height, std::uint32_t dpi,
                                  widget_bridge::Rect const* rects, std::uint64_t const* ids,
                                  std::int32_t const* anchors, std::uint32_t count, std::int32_t individual,
                                  std::int32_t light, std::int32_t hover,
                                  std::int32_t locked, std::int32_t desiredColumn) noexcept {
    auto* publisher = static_cast<widget_bridge::Publisher*>(value);
    return publisher && publisher->Publish(reinterpret_cast<DWORD const*>(pixels), width, height, dpi,
                                           rects, ids, anchors, count, individual != 0,
                                           light != 0, hover != 0, locked != 0, desiredColumn);
}

std::int32_t bhx_publisher_state(void* value) noexcept {
    auto* publisher = static_cast<widget_bridge::Publisher*>(value);
    return publisher ? static_cast<std::int32_t>(publisher->State()) : 3;
}

std::int32_t bhx_publisher_ready_slot(void* value, widget_bridge::Rect* rect) noexcept {
    auto* publisher = static_cast<widget_bridge::Publisher*>(value);
    if (!publisher || !rect) return 0;
    RECT slot{};
    if (!publisher->ReadReadySlot(slot)) return 0;
    *rect = {slot.left, slot.top, slot.right, slot.bottom};
    return 1;
}

} // extern "C"
