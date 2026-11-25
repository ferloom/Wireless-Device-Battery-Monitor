#pragma once

#include <windows.h>
#include <robuffer.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.Storage.Streams.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Core.h>
#include <winrt/Windows.UI.Input.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Input.h>
#include <winrt/Windows.UI.Xaml.Media.Imaging.h>

#include "WidgetReadiness.h"
#include "../publisher/widget_bridge_protocol.h"

#include <vector>
#include <functional>
#include <utility>

namespace batteryhub::xaml {

using InputEvent = widget_bridge::InputKind;
constexpr UINT kInputMessage = WM_APP + 0x45A;

struct SurfaceDiagnostics {
    std::uint64_t sessionId = 0, pixelHash = 0;
    std::uint32_t generation = 0, widthPx = 0, heightPx = 0, dpi = 96, invalidateCount = 0;
    std::uint32_t dispatcherThreadId = 0, inspectThreadId = 0, renderPassCount = 0;
    std::uint32_t renderCallbackThreadId = 0;
    std::uint32_t renderSubscriptionCount = 0, renderCallbackCount = 0, renderUnsubscribeCount = 0;
    std::uint32_t rootLoadedEventCount = 0, imageLoadedEventCount = 0;
    std::uint32_t rootUnloadedEventCount = 0, imageUnloadedEventCount = 0;
    std::uint32_t rootSizeChangedCount = 0, imageSizeChangedCount = 0, layoutUpdatedCount = 0;
    double rootWidthDip = 0, rootHeightDip = 0, rootActualWidth = 0, rootActualHeight = 0;
    double imageWidthDip = 0, imageHeightDip = 0, imageActualWidth = 0, imageActualHeight = 0;
    double xamlRootWidth = 0, xamlRootHeight = 0, rasterizationScale = 0;
    double rootOpacity = 0, imageOpacity = 0;
    std::uint64_t renderLatencyMs = 0;
    std::uintptr_t occludingWindow = 0;
    std::uint32_t occludingPid = 0, taskbarVisibleSampleCount = 0;
    bool imageSourceValid = false, imageSourceInterfacePointerEqual = false, imageSourceMatchesBitmap = false;
    bool rootLoaded = false, imageLoaded = false, rootVisible = false, imageVisible = false;
    bool xamlRootValid = false, xamlRootHostVisible = false, taskbarWindowVisible = false;
    bool hostGateChecked = false, hostGateReady = false, surfaceUncovered = false;
    bool hostPresentationCommitted = false;
    bool hitTestVisible = false, rootClipped = false, imageClipped = false;
    bool frameValid = false, pixelCopyMatches = false, renderedAfterVisible = false, surfaceVisibleRequested = false;
    bool renderingSubscribed = false, renderCallbackThreadMatches = false;
    bool renderObserverFailed = false, presentationPending = false, layoutReady = false, presentationReady = false;
    bool dispatcherHasThreadAccess = false, dispatcherThreadMatches = false;
    bool dimensionsMatch = false, surfaceReady = false;
    wchar_t const* firstFailure = L"no-frame";
    wchar_t const* hostGateFailure = L"not-checked";
};

class WidgetSurfaceView {
public:
    WidgetSurfaceView();
    ~WidgetSurfaceView();

    winrt::Windows::UI::Xaml::Controls::Grid Root() const { return m_root; }
    void SetInputTarget(HWND window, UINT message = kInputMessage) noexcept;
    HRESULT Update(std::uint32_t const* bgra, std::uint32_t width, std::uint32_t height,
                   std::uint32_t dpi, widget_bridge::Rect const* rects,
                   std::uint64_t const* itemIds, std::int32_t const* itemAnchors,
                   bool individual,
                   std::uint32_t itemCount, std::uint64_t sessionId = 0,
                   std::uint32_t generation = 0) noexcept;
    HRESULT GetScreenRect(HWND taskbarWindow, RECT& screenRect) noexcept;
    HRESULT SetSurfaceVisible(bool visible) noexcept;
    void SetDragEnabled(bool enabled) noexcept { m_dragEnabled = enabled; m_moduleDragEnabled = enabled; }
    winrt::Windows::UI::Xaml::Controls::Grid ModuleRoot(std::uint64_t id) const noexcept;
    void SetModuleDragCallbacks(std::function<int(std::size_t, double)> preview,
                                std::function<int(std::size_t)> commit,
                                std::function<void(std::size_t)> cancel) {
        m_previewModuleDrag = std::move(preview); m_commitModuleDrag = std::move(commit);
        m_cancelModuleDrag = std::move(cancel);
    }
    void SetDragCallbacks(std::function<int(double)> preview, std::function<int()> commit,
                          std::function<void()> cancel, std::function<void(wchar_t const*)> trace) {
        m_previewDrag = std::move(preview); m_commitDrag = std::move(commit);
        m_cancelDrag = std::move(cancel); m_traceDrag = std::move(trace);
    }
    SurfaceDiagnostics Inspect() const noexcept;
    bool IsSurfaceReady() const noexcept;

private:
    struct ModuleView {
        std::uint64_t id = 0;
        std::uint32_t index = 0;
        widget_bridge::Rect rect{};
        winrt::Windows::UI::Xaml::Controls::Grid root{ nullptr };
        winrt::Windows::UI::Xaml::Controls::Image image{ nullptr };
        winrt::Windows::UI::Xaml::Media::Imaging::WriteableBitmap bitmap{ nullptr };
        widget_bridge::Rect bitmapRect{};
        bool captured = false, dragging = false;
        int pressedKind = 0;
        double pressX = 0, pressY = 0;
    };
    void SyncModules(std::uint64_t const* ids, std::uint32_t count);
    void LayoutModules();
    int ModuleIndex(std::uint64_t id) const noexcept;
    void SendModuleAnchor(std::size_t item, int anchor) const noexcept;
    std::pair<double, double> ModuleSurfacePoint(ModuleView const& module, double x, double y) const noexcept;
    int HitTest(double xDip, double yDip) const noexcept;
    void Send(InputEvent input, int item, double xDip = 0, double yDip = 0) const noexcept;
    void SendModuleHover(std::size_t item, std::uint64_t id) const noexcept;
    void SendModuleLeave(std::size_t item) const noexcept;
    void EnsureRenderingObservation() noexcept;
    void StopRenderingObservation() noexcept;

    winrt::Windows::UI::Xaml::Controls::Grid m_root{ nullptr };
    winrt::Windows::UI::Xaml::Controls::Image m_image{ nullptr };
    winrt::Windows::UI::Xaml::Media::Imaging::WriteableBitmap m_bitmap{ nullptr };
    winrt::event_token m_renderingToken{};
    winrt::event_token m_rootLoadedToken{}, m_rootUnloadedToken{}, m_rootSizeChangedToken{}, m_rootLayoutUpdatedToken{};
    winrt::event_token m_imageLoadedToken{}, m_imageUnloadedToken{}, m_imageSizeChangedToken{};
    std::vector<widget_bridge::Rect> m_rects;
    std::vector<std::uint64_t> m_ids;
    std::vector<std::int32_t> m_anchors;
    std::vector<ModuleView> m_modules;
    HWND m_inputWindow = nullptr;
    HWND m_taskbarWindow = nullptr;
    UINT m_inputMessage = kInputMessage;
    std::uint32_t m_width = 0, m_height = 0, m_dpi = 96;
    std::uint64_t m_sessionId = 0, m_pixelHash = 0;
    std::uint32_t m_generation = 0, m_invalidateCount = 0;
    std::uint32_t m_dispatcherThreadId = 0, m_renderPassCount = 0;
    std::uint32_t m_renderCallbackThreadId = 0;
    std::uint32_t m_renderSubscriptionCount = 0, m_renderCallbackCount = 0, m_renderUnsubscribeCount = 0;
    std::uint32_t m_rootLoadedEventCount = 0, m_imageLoadedEventCount = 0;
    std::uint32_t m_rootUnloadedEventCount = 0, m_imageUnloadedEventCount = 0;
    std::uint32_t m_rootSizeChangedCount = 0, m_imageSizeChangedCount = 0, m_layoutUpdatedCount = 0;
    std::uint64_t m_renderSubscribedAt = 0, m_renderLatencyMs = 0;
    mutable std::uintptr_t m_occludingWindow = 0;
    mutable std::uint32_t m_occludingPid = 0, m_taskbarVisibleSampleCount = 0;
    mutable bool m_taskbarWindowVisible = false, m_hostGateChecked = false;
    mutable bool m_hostGateReady = false, m_surfaceUncovered = false;
    mutable bool m_hostPresentationCommitted = false, m_haveHostVisibility = false, m_lastHostVisible = false;
    mutable wchar_t const* m_hostGateFailure = L"not-checked";
    PresentationFence m_presentation;
    bool m_renderObserverFailed = false;
    int m_hoveredItem = -2;
    int m_pressedKind = 0;
    double m_pointerX = 0, m_pointerY = 0;
    bool m_pointerInside = false, m_frameValid = false, m_pixelCopyMatches = false;
    bool m_renderingSubscribed = false;
    bool m_dragEnabled = false, m_dragArmed = false, m_dragging = false;
    double m_pressX = 0, m_pressY = 0;
    std::function<int(double)> m_previewDrag;
    std::function<int()> m_commitDrag;
    std::function<void()> m_cancelDrag;
    std::function<void(wchar_t const*)> m_traceDrag;
    std::function<int(std::size_t, double)> m_previewModuleDrag;
    std::function<int(std::size_t)> m_commitModuleDrag;
    std::function<void(std::size_t)> m_cancelModuleDrag;
    bool m_individual = false, m_moduleDragEnabled = false;
    bool m_loggedFirstDragMove = false, m_pointerCaptureSucceeded = false;
};

} // namespace batteryhub::xaml
