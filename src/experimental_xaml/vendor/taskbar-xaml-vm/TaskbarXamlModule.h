#pragma once

#include <windows.h>
#include "../publisher/widget_bridge_protocol.h"

// This is a layout engine only. A build-specific Explorer adapter must call
// it on the taskbar's XAML thread with the live SystemTrayFrameGrid ABI pointer.
// It deliberately does not discover Explorer internals or query HID.
extern "C" {
    struct BatteryHubTaskbarXamlStatus {
        unsigned int consecutiveFailures;
        int attached;
        int disabled;
    };

    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlAttach(void* trayAbi, BOOL primaryHorizontalTaskbar);
    // Accepts the XamlRoot.Content() UIElement supplied by a separately
    // verified private taskbar bridge. Finds the unique tray and chevron.
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlAttachTree(void* rootContentAbi, BOOL primaryHorizontalTaskbar);
    // Production gate: confirms the supplied HWND is the primary horizontal
    // Shell_TrayWnd before using the visual-tree adapter.
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlAttachVerifiedTaskbarTree(void* rootContentAbi, HWND taskbarWindow);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlDetach();
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlTreeRebuilt(void* newTrayAbi, BOOL primaryHorizontalTaskbar);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlTreeRebuiltTree(void* rootContentAbi, BOOL primaryHorizontalTaskbar);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlTreeRebuiltVerifiedTaskbarTree(void* rootContentAbi, HWND taskbarWindow);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlResetCircuitBreaker();
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlGetStatus(BatteryHubTaskbarXamlStatus* status);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlSetLogPath(wchar_t const* absolutePath);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlSetInputTarget(HWND window, UINT message);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlUpdateSurface(
        std::uint32_t const* premultipliedBgra, std::uint32_t widthPx, std::uint32_t heightPx,
        std::uint32_t dpi, widget_bridge::Rect const* itemRects, std::uint64_t const* itemIds,
        std::int32_t const* itemAnchors, std::uint32_t itemCount, BOOL individual,
        std::uint64_t sessionId, std::uint32_t generation);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlSetWidgetLayout(
        BOOL individual, std::uint64_t const* itemIds, std::int32_t const* itemAnchors,
        std::uint32_t itemCount, std::int32_t desiredGridColumn, BOOL positionLocked);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlSetWidgetPosition(std::int32_t desiredGridColumn, BOOL positionLocked);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlGetWidgetScreenRect(HWND taskbarWindow, RECT* screenRect);
    __declspec(dllexport) HRESULT WINAPI BatteryHubTaskbarXamlSetWidgetVisible(BOOL visible);
}
