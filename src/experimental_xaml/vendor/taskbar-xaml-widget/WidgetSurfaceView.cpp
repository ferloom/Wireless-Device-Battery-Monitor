#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#undef GetCurrentTime

#include "WidgetSurfaceView.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>

using namespace winrt;
using namespace winrt::Windows::UI::Xaml;
using namespace winrt::Windows::UI::Xaml::Controls;
using namespace winrt::Windows::UI::Xaml::Media;
using namespace winrt::Windows::UI::Xaml::Media::Imaging;

namespace batteryhub::xaml {

namespace {
template <typename Left, typename Right>
bool SameComIdentity(Left const& left, Right const& right) noexcept {
    auto* leftAbi = reinterpret_cast<::IUnknown*>(get_abi(left));
    auto* rightAbi = reinterpret_cast<::IUnknown*>(get_abi(right));
    if (!leftAbi || !rightAbi) return false;
    ::IUnknown* leftIdentity = nullptr;
    ::IUnknown* rightIdentity = nullptr;
    const HRESULT leftHr = leftAbi->QueryInterface(IID_IUnknown, reinterpret_cast<void**>(&leftIdentity));
    const HRESULT rightHr = rightAbi->QueryInterface(IID_IUnknown, reinterpret_cast<void**>(&rightIdentity));
    const bool same = SUCCEEDED(leftHr) && SUCCEEDED(rightHr) && leftIdentity == rightIdentity;
    if (leftIdentity) leftIdentity->Release();
    if (rightIdentity) rightIdentity->Release();
    return same;
}

bool SampleCoveredByWindow(HWND expected, RECT const& rect, HWND& firstOccluder,
                           std::uint32_t& samples) noexcept {
    const LONG width = rect.right - rect.left;
    const LONG height = rect.bottom - rect.top;
    if (!expected || width <= 0 || height <= 0) return true;
    const LONG dx = width > 4 ? 2 : 0;
    const LONG dy = height > 4 ? 2 : 0;
    const LONG xs[] = { rect.left + dx, rect.left + width / 2, rect.right - 1 - dx };
    const LONG ys[] = { rect.top + dy, rect.top + height / 2, rect.bottom - 1 - dy };
    for (LONG y : ys) for (LONG x : xs) {
        HWND hit = WindowFromPoint(POINT{ x, y });
        ++samples;
        const HWND root = hit ? GetAncestor(hit, GA_ROOT) : nullptr;
        const HWND rootOwner = hit ? GetAncestor(hit, GA_ROOTOWNER) : nullptr;
        if (!hit || (hit != expected && !IsChild(expected, hit) && root != expected && rootOwner != expected)) {
            firstOccluder = hit;
            return true;
        }
    }
    return false;
}
}

WidgetSurfaceView::WidgetSurfaceView() {
    m_dispatcherThreadId = GetCurrentThreadId();
    m_root = Grid();
    m_root.Opacity(0.0);
    m_root.IsHitTestVisible(true);
    m_root.Background(winrt::Windows::UI::Xaml::Media::SolidColorBrush(
        winrt::Windows::UI::Color{1, 0, 0, 0}));
    m_image = Image();
    m_image.Stretch(winrt::Windows::UI::Xaml::Media::Stretch::Fill);
    m_root.Children().Append(m_image);

    m_rootLoadedToken = m_root.Loaded([this](auto const&, auto const&) { ++m_rootLoadedEventCount; });
    m_rootUnloadedToken = m_root.Unloaded([this](auto const&, auto const&) { ++m_rootUnloadedEventCount; });
    m_rootSizeChangedToken = m_root.SizeChanged([this](auto const&, auto const&) { ++m_rootSizeChangedCount; });
    m_rootLayoutUpdatedToken = m_root.LayoutUpdated([this](auto const&, auto const&) {
        // This counter is diagnostic only; LayoutUpdated can run on every Explorer layout pass.
        ++m_layoutUpdatedCount;
        m_root.LayoutUpdated(m_rootLayoutUpdatedToken);
        m_rootLayoutUpdatedToken = {};
    });
    m_imageLoadedToken = m_image.Loaded([this](auto const&, auto const&) { ++m_imageLoadedEventCount; });
    m_imageUnloadedToken = m_image.Unloaded([this](auto const&, auto const&) { ++m_imageUnloadedEventCount; });
    m_imageSizeChangedToken = m_image.SizeChanged([this](auto const&, auto const&) { ++m_imageSizeChangedCount; });

    m_root.PointerEntered([this](winrt::Windows::Foundation::IInspectable const&, auto const& args) {
        auto point = args.GetCurrentPoint(m_root);
        m_pointerX = point.Position().X;
        m_pointerY = point.Position().Y;
        m_pointerInside = true;
        m_hoveredItem = HitTest(point.Position().X, point.Position().Y);
        Send(InputEvent::Enter, m_hoveredItem, point.Position().X, point.Position().Y);
    });
    m_root.PointerMoved([this](winrt::Windows::Foundation::IInspectable const&, auto const& args) {
        auto point = args.GetCurrentPoint(m_root);
        m_pointerX = point.Position().X;
        m_pointerY = point.Position().Y;
        m_pointerInside = true;
        if (m_dragArmed && !m_dragging && m_pointerCaptureSucceeded) {
            const double thresholdX = (std::max)(1.0, GetSystemMetrics(SM_CXDRAG) * 48.0 / (std::max)(48u, m_dpi));
            const double thresholdY = (std::max)(1.0, GetSystemMetrics(SM_CYDRAG) * 48.0 / (std::max)(48u, m_dpi));
            if (std::abs(point.Position().X - m_pressX) >= thresholdX ||
                std::abs(point.Position().Y - m_pressY) >= thresholdY) {
                m_dragging = true;
                m_dragArmed = false;
                m_hoveredItem = -2;
                Send(InputEvent::Leave, -1);
                if (m_traceDrag) m_traceDrag(L"drag-threshold-crossed");
            }
        }
        if (m_dragging) {
            const int candidate = m_previewDrag ? m_previewDrag(point.Position().X) : -1;
            if (!m_loggedFirstDragMove && m_traceDrag) {
                wchar_t line[160]{};
                swprintf_s(line, L"drag-first-move candidate=%d captured=%u xDip=%.1f",
                           candidate, m_pointerCaptureSucceeded ? 1u : 0u, point.Position().X);
                m_traceDrag(line);
                m_loggedFirstDragMove = true;
            }
            args.Handled(true);
            return;
        }
        int item = HitTest(point.Position().X, point.Position().Y);
        if (item != m_hoveredItem) {
            m_hoveredItem = item;
            Send(InputEvent::Move, item, point.Position().X, point.Position().Y);
        }
    });
    m_root.PointerExited([this](winrt::Windows::Foundation::IInspectable const&, auto const&) {
        if (m_dragArmed || m_dragging) return;
        if (m_hoveredItem != -2) {
            m_hoveredItem = -2;
            m_pointerInside = false;
            Send(InputEvent::Leave, -1);
        }
    });
    m_root.PointerPressed([this](winrt::Windows::Foundation::IInspectable const&, auto const& args) {
        auto properties = args.GetCurrentPoint(m_root).Properties();
        m_pressedKind = properties.IsLeftButtonPressed() ? static_cast<int>(InputEvent::LeftClick)
                       : properties.IsRightButtonPressed() ? static_cast<int>(InputEvent::RightClick) : 0;
        m_pointerCaptureSucceeded = false;
        m_loggedFirstDragMove = false;
        const auto point = args.GetCurrentPoint(m_root).Position();
        m_pressX = point.X;
        m_pressY = point.Y;
        m_dragArmed = m_pressedKind == static_cast<int>(InputEvent::LeftClick) && m_dragEnabled;
        if (m_pressedKind) m_pointerCaptureSucceeded = m_root.CapturePointer(args.Pointer());
        if (!m_pointerCaptureSucceeded) m_dragArmed = false;
        if (m_traceDrag) {
            wchar_t line[160]{};
            swprintf_s(line, L"drag-press kind=%d enabled=%u captured=%u armed=%u", m_pressedKind,
                       m_dragEnabled ? 1u : 0u, m_pointerCaptureSucceeded ? 1u : 0u, m_dragArmed ? 1u : 0u);
            m_traceDrag(line);
        }
        args.Handled(true);
    });
    m_root.PointerReleased([this](winrt::Windows::Foundation::IInspectable const&, auto const& args) {
        const int pressedKind = m_pressedKind;
        m_pressedKind = 0;
        m_dragArmed = false;
        if (m_dragging) {
            m_dragging = false;
            const int column = m_commitDrag ? m_commitDrag() : -1;
            if (column >= widget_bridge::kMinGridAnchor && column < static_cast<int>(widget_bridge::kMaxGridColumns)) Send(InputEvent::DragCommit, column);
            else if (m_cancelDrag) m_cancelDrag();
            if (m_traceDrag) {
                wchar_t line[128]{};
                swprintf_s(line, L"drag-release column=%d captured=%u", column,
                           m_pointerCaptureSucceeded ? 1u : 0u);
                m_traceDrag(line);
            }
            auto point = args.GetCurrentPoint(m_root);
            m_pointerX = point.Position().X;
            m_pointerY = point.Position().Y;
            m_pointerInside = true;
            m_hoveredItem = HitTest(m_pointerX, m_pointerY);
            Send(InputEvent::Enter, m_hoveredItem, m_pointerX, m_pointerY);
        } else if (pressedKind) {
            auto point = args.GetCurrentPoint(m_root);
            Send(static_cast<InputEvent>(pressedKind), HitTest(point.Position().X, point.Position().Y),
                 point.Position().X, point.Position().Y);
        }
        if (m_pointerCaptureSucceeded) m_root.ReleasePointerCapture(args.Pointer());
        m_pointerCaptureSucceeded = false;
        args.Handled(true);
    });
    m_root.PointerCaptureLost([this](winrt::Windows::Foundation::IInspectable const&, auto const&) {
        if (!m_dragging && !m_dragArmed) return;
        const bool wasDragging = m_dragging;
        m_dragArmed = false;
        m_dragging = false;
        m_pointerCaptureSucceeded = false;
        m_pressedKind = 0;
        if (wasDragging && m_cancelDrag) m_cancelDrag();
        if (m_traceDrag) m_traceDrag(L"drag-capture-lost");
        m_hoveredItem = m_pointerInside ? HitTest(m_pointerX, m_pointerY) : -2;
        Send(m_pointerInside ? InputEvent::Enter : InputEvent::Leave, m_hoveredItem,
             m_pointerX, m_pointerY);
    });
}

WidgetSurfaceView::~WidgetSurfaceView() {
    StopRenderingObservation();
    try {
        if (m_root) {
            m_root.Loaded(m_rootLoadedToken);
            m_root.Unloaded(m_rootUnloadedToken);
            m_root.SizeChanged(m_rootSizeChangedToken);
            if (m_rootLayoutUpdatedToken.value) m_root.LayoutUpdated(m_rootLayoutUpdatedToken);
        }
        if (m_image) {
            m_image.Loaded(m_imageLoadedToken);
            m_image.Unloaded(m_imageUnloadedToken);
            m_image.SizeChanged(m_imageSizeChangedToken);
        }
    } catch (...) { }
}

void WidgetSurfaceView::SetInputTarget(HWND window, UINT message) noexcept {
    m_inputWindow = window;
    m_inputMessage = message ? message : kInputMessage;
}

winrt::Windows::UI::Xaml::Controls::Grid WidgetSurfaceView::ModuleRoot(std::uint64_t id) const noexcept {
    const int index = ModuleIndex(id);
    return index >= 0 ? m_modules[static_cast<std::size_t>(index)].root : nullptr;
}

int WidgetSurfaceView::ModuleIndex(std::uint64_t id) const noexcept {
    for (std::size_t i = 0; i < m_modules.size(); ++i)
        if (m_modules[i].id == id) return static_cast<int>(i);
    return -1;
}

std::pair<double, double> WidgetSurfaceView::ModuleSurfacePoint(
    ModuleView const& module, double x, double y) const noexcept {
    const double scale = 96.0 / (std::max)(48u, m_dpi);
    return { module.rect.left * scale + x, module.rect.top * scale + y };
}

void WidgetSurfaceView::SendModuleAnchor(std::size_t item, int anchor) const noexcept {
    if (!m_inputWindow || !IsWindow(m_inputWindow) || !m_inputMessage || item >= widget_bridge::kMaxItems) return;
    const auto packed = widget_bridge::EncodeInput(InputEvent::ModuleDragCommit, static_cast<int>(item));
    if (packed) PostMessageW(m_inputWindow, m_inputMessage, packed, static_cast<LPARAM>(anchor));
}

void WidgetSurfaceView::SendModuleHover(std::size_t item, std::uint64_t id) const noexcept {
    if (!m_inputWindow || !IsWindow(m_inputWindow) || !m_inputMessage || item >= widget_bridge::kMaxItems) return;
    const auto packed = widget_bridge::EncodeInput(InputEvent::ModuleHover, static_cast<int>(item));
    const int index = ModuleIndex(id);
    if (!packed || index < 0) return;
    const auto& module = m_modules[static_cast<std::size_t>(index)];
    int screenX = 0;
    try {
        const auto point = module.root.TransformToVisual(nullptr).TransformPoint(
            {static_cast<float>(module.root.ActualWidth() / 2.0), static_cast<float>(module.root.ActualHeight() / 2.0)});
        POINT origin{0, 0};
        if (m_taskbarWindow && IsWindow(m_taskbarWindow)) {
            SetLastError(ERROR_SUCCESS);
            if (!MapWindowPoints(m_taskbarWindow, nullptr, &origin, 1) && GetLastError() != ERROR_SUCCESS) return;
        }
        screenX = origin.x + static_cast<int>(std::lround(point.X * static_cast<double>(m_dpi) / 96.0));
    } catch (...) { return; }
    PostMessageW(m_inputWindow, m_inputMessage, packed, static_cast<LPARAM>(screenX));
}

void WidgetSurfaceView::SendModuleLeave(std::size_t item) const noexcept {
    if (!m_inputWindow || !IsWindow(m_inputWindow) || !m_inputMessage || item >= widget_bridge::kMaxItems) return;
    const auto packed = widget_bridge::EncodeInput(InputEvent::ModuleLeave, static_cast<int>(item));
    if (packed) PostMessageW(m_inputWindow, m_inputMessage, packed, 0);
}

void WidgetSurfaceView::SyncModules(std::uint64_t const* ids, std::uint32_t count) {
    std::vector<std::uint64_t> nextIds;
    if (count) nextIds.assign(ids, ids + count);
    if (!m_individual) {
        m_modules.clear();
        m_ids = std::move(nextIds);
        return;
    }
    if (m_ids != nextIds || m_modules.size() != count) {
        auto old = std::move(m_modules);
        m_modules.clear();
        m_modules.reserve(count);
        for (std::uint32_t i = 0; i < count; ++i) {
            const auto id = nextIds[i];
            auto found = std::find_if(old.begin(), old.end(), [id](auto const& module) { return module.id == id; });
            if (found != old.end()) {
                found->index = i;
                found->rect = m_rects[i];
                m_modules.push_back(std::move(*found));
                continue;
            }
            ModuleView module{};
            module.id = id;
            module.index = i;
            module.rect = m_rects[i];
            module.root = Grid();
            wchar_t name[64]{};
            swprintf_s(name, L"BatteryHubModule_%016llX", static_cast<unsigned long long>(id));
            module.root.Name(name);
            module.root.IsHitTestVisible(true);
            module.root.Background(SolidColorBrush(winrt::Windows::UI::Color{1, 0, 0, 0}));
            module.root.Clip(RectangleGeometry());
            module.image = Image();
            module.image.Stretch(Stretch::Fill);
            module.image.IsHitTestVisible(false);
            module.image.HorizontalAlignment(HorizontalAlignment::Left);
            module.image.VerticalAlignment(VerticalAlignment::Top);
            module.root.Children().Append(module.image);

            module.root.PointerEntered([this, id](auto const&, auto const& args) {
                const int n = ModuleIndex(id); if (n < 0) return;
                auto& m = m_modules[static_cast<std::size_t>(n)];
                SendModuleHover(m.index, id);
            });
            module.root.PointerMoved([this, id](auto const&, auto const& args) {
                const int n = ModuleIndex(id); if (n < 0) return;
                auto& m = m_modules[static_cast<std::size_t>(n)];
                const auto point = args.GetCurrentPoint(m.root).Position();
                if (m.dragging) {
                    if (m_previewModuleDrag) (void)m_previewModuleDrag(m.index, point.X);
                    args.Handled(true);
                    return;
                }
                if (m.captured && m_moduleDragEnabled) {
                    const double tx = (std::max)(1.0, GetSystemMetrics(SM_CXDRAG) * 48.0 / (std::max)(48u, m_dpi));
                    const double ty = (std::max)(1.0, GetSystemMetrics(SM_CYDRAG) * 48.0 / (std::max)(48u, m_dpi));
                    if (std::abs(point.X - m.pressX) >= tx || std::abs(point.Y - m.pressY) >= ty) {
                        m.dragging = true;
                        Send(InputEvent::Leave, -1);
                        if (m_previewModuleDrag) (void)m_previewModuleDrag(m.index, point.X);
                    }
                }
            });
            module.root.PointerExited([this, id](auto const&, auto const&) {
                const int n = ModuleIndex(id); if (n < 0) return;
                auto const& m = m_modules[static_cast<std::size_t>(n)];
                if (!m.captured) SendModuleLeave(m.index);
            });
            module.root.PointerPressed([this, id](auto const&, auto const& args) {
                const int n = ModuleIndex(id); if (n < 0) return;
                auto& m = m_modules[static_cast<std::size_t>(n)];
                const auto point = args.GetCurrentPoint(m.root);
                const auto properties = point.Properties();
                m.pressedKind = properties.IsRightButtonPressed() ? static_cast<int>(InputEvent::RightClick)
                               : properties.IsLeftButtonPressed() ? static_cast<int>(InputEvent::LeftClick) : 0;
                m.pressX = point.Position().X; m.pressY = point.Position().Y;
                m.dragging = false; m.captured = m_moduleDragEnabled && m.pressedKind == static_cast<int>(InputEvent::LeftClick);
                if (m.captured) m.captured = m.root.CapturePointer(args.Pointer());
                if (m.pressedKind == static_cast<int>(InputEvent::RightClick)) {
                    const auto surface = ModuleSurfacePoint(m, m.pressX, m.pressY);
                    Send(InputEvent::RightClick, static_cast<int>(m.index), surface.first, surface.second);
                }
                args.Handled(m.pressedKind != 0);
            });
            module.root.PointerReleased([this, id](auto const&, auto const& args) {
                const int n = ModuleIndex(id); if (n < 0) return;
                auto& m = m_modules[static_cast<std::size_t>(n)];
                if (m.dragging) {
                    m.dragging = false;
                    const int anchor = m_commitModuleDrag ? m_commitModuleDrag(m.index) : -1;
                    if (anchor != -1) SendModuleAnchor(m.index, anchor);
                    else if (m_cancelModuleDrag) m_cancelModuleDrag(m.index);
                    args.Handled(true);
                } else if (m.pressedKind == static_cast<int>(InputEvent::LeftClick)) {
                    const auto point = args.GetCurrentPoint(m.root).Position();
                    const auto surface = ModuleSurfacePoint(m, point.X, point.Y);
                    Send(InputEvent::LeftClick, static_cast<int>(m.index), surface.first, surface.second);
                }
                const bool captured = m.captured;
                m.captured = false; m.dragging = false; m.pressedKind = 0;
                if (captured) m.root.ReleasePointerCapture(args.Pointer());
            });
            module.root.PointerCaptureLost([this, id](auto const&, auto const&) {
                const int n = ModuleIndex(id); if (n < 0) return;
                auto& m = m_modules[static_cast<std::size_t>(n)];
                if (m.dragging && m_cancelModuleDrag) m_cancelModuleDrag(m.index);
                if (m.dragging) Send(InputEvent::DeviceDragCancel, static_cast<int>(m.index));
                m.captured = false; m.dragging = false; m.pressedKind = 0;
            });
            m_modules.push_back(std::move(module));
        }
        m_ids = std::move(nextIds);
    } else {
        std::size_t moduleIndex = 0;
        for (std::uint32_t i = 0; i < count; ++i) {
            auto& module = m_modules[moduleIndex++];
            module.index = i;
            module.rect = m_rects[i];
        }
    }
    LayoutModules();
}

void WidgetSurfaceView::LayoutModules() {
    if (!m_bitmap || !m_dpi) return;
    for (auto& module : m_modules) {
        const double scale = 96.0 / m_dpi;
        const double x = module.rect.left * scale, y = module.rect.top * scale;
        const double width = (module.rect.right - module.rect.left) * scale;
        const double height = (module.rect.bottom - module.rect.top) * scale;
        module.root.Width(width); module.root.Height(height);
        module.root.Opacity(m_presentation.visible ? 1.0 : 0.0);
        auto clip = module.root.Clip().try_as<RectangleGeometry>();
        if (clip) clip.Rect(winrt::Windows::Foundation::Rect{0.0f, 0.0f, static_cast<float>(width), static_cast<float>(height)});
        const auto pixelWidth = static_cast<int>(module.rect.right - module.rect.left);
        const auto pixelHeight = static_cast<int>(module.rect.bottom - module.rect.top);
        if (!module.bitmap || module.bitmap.PixelWidth() != pixelWidth || module.bitmap.PixelHeight() != pixelHeight) {
            module.bitmap = WriteableBitmap(pixelWidth, pixelHeight);
            module.image.Source(module.bitmap);
        }
        module.bitmapRect = module.rect;
        module.image.Width(width); module.image.Height(height);
    }
}

int WidgetSurfaceView::HitTest(double xDip, double yDip) const noexcept {
    if (!m_width || !m_height || !m_dpi) return -1;
    const auto x = static_cast<std::int32_t>(xDip * m_dpi / 96.0);
    const auto y = static_cast<std::int32_t>(yDip * m_dpi / 96.0);
    for (std::size_t i = 0; i < m_rects.size(); ++i) {
        auto const& r = m_rects[i];
        if (x >= r.left && x < r.right && y >= r.top && y < r.bottom) return static_cast<int>(i);
    }
    return -1;
}

void WidgetSurfaceView::Send(InputEvent input, int item, double xDip, double yDip) const noexcept {
    if (!m_inputWindow || !IsWindow(m_inputWindow) || !m_inputMessage) return;
    const auto packed = widget_bridge::EncodeInput(input, item);
    if (!packed) return;
    const auto x = static_cast<std::uint16_t>(std::clamp(xDip * m_dpi / 96.0, 0.0, 65535.0));
    const auto y = static_cast<std::uint16_t>(std::clamp(yDip * m_dpi / 96.0, 0.0, 65535.0));
    const auto point = static_cast<std::uint32_t>(x) | (static_cast<std::uint32_t>(y) << 16);
    PostMessageW(m_inputWindow, m_inputMessage, packed, static_cast<LPARAM>(point));
}

HRESULT WidgetSurfaceView::Update(std::uint32_t const* bgra, std::uint32_t width,
                                  std::uint32_t height, std::uint32_t dpi,
                                  widget_bridge::Rect const* rects,
                                  std::uint64_t const* itemIds, std::int32_t const* itemAnchors,
                                  bool individual, std::uint32_t itemCount, std::uint64_t sessionId,
                                  std::uint32_t generation) noexcept {
    const bool hadValidFrame = m_frameValid;
    const bool scaleChanged = dpi != m_dpi;
    const bool modeChanged = individual != m_individual;
    bool moduleIdsChanged = individual && (itemCount != m_ids.size() || itemCount != m_modules.size());
    bool moduleAnchorsChanged = individual && itemCount != m_anchors.size();
    for (std::uint32_t i = 0; individual && !moduleIdsChanged && i < itemCount; ++i)
        moduleIdsChanged = m_ids[i] != itemIds[i];
    for (std::uint32_t i = 0; individual && !moduleAnchorsChanged && i < itemCount; ++i)
        moduleAnchorsChanged = m_anchors[i] != itemAnchors[i];
    const bool moduleLayoutChanged = modeChanged || moduleIdsChanged || moduleAnchorsChanged;
    m_frameValid = false;
    if (!width || !height || !dpi || width > UINT32_MAX / 4u / height) return E_INVALIDARG;
    const std::uint64_t pixelCount = static_cast<std::uint64_t>(width) * height;
    const std::uint64_t bytes = pixelCount * 4u;
    if (!bgra || (itemCount && (!rects || !itemIds || !itemAnchors)) ||
        !widget_bridge::ValidSurface(width, height, static_cast<std::uint32_t>(bytes), itemCount, dpi)) return E_INVALIDARG;
    if (!widget_bridge::ValidPremultipliedBgra(bgra, static_cast<std::size_t>(pixelCount))) return E_INVALIDARG;
    for (std::uint32_t i = 0; i < itemCount; ++i) {
        if (!widget_bridge::ValidRect(rects[i], width, height) || itemAnchors[i] < widget_bridge::kMinGridAnchor ||
            itemAnchors[i] >= static_cast<std::int32_t>(widget_bridge::kMaxGridColumns)) return E_INVALIDARG;
        if (itemIds[i]) for (std::uint32_t j = 0; j < i; ++j) if (itemIds[j] == itemIds[i]) return E_INVALIDARG;
    }
    try {
        m_sessionId = sessionId;
        m_generation = generation;
        m_pixelCopyMatches = false;
        const bool resize = !m_bitmap || width != m_width || height != m_height;
        if (resize) {
            m_bitmap = WriteableBitmap(static_cast<int>(width), static_cast<int>(height));
            m_image.Source(m_bitmap);
            m_width = width;
            m_height = height;
            m_hoveredItem = -2;
        }
        m_dpi = dpi;
        if (itemCount) m_rects.assign(rects, rects + itemCount);
        else m_rects.clear();
        if (itemCount) {
            m_anchors.assign(itemAnchors, itemAnchors + itemCount);
        } else m_anchors.clear();
        m_individual = individual;
        SyncModules(itemIds, itemCount);
        if (m_pointerInside) {
            const int item = HitTest(m_pointerX, m_pointerY);
            if (item != m_hoveredItem) {
                m_hoveredItem = item;
                Send(InputEvent::Move, item, m_pointerX, m_pointerY);
            }
        }
        const auto buffer = m_bitmap.PixelBuffer();
        if (buffer.Length() != static_cast<std::uint32_t>(bytes)) return HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
        auto access = buffer.as<::Windows::Storage::Streams::IBufferByteAccess>();
        byte* destination = nullptr;
        check_hresult(access->Buffer(&destination));
        const auto sourceHash = widget_bridge::PixelHash(bgra, static_cast<std::size_t>(width) * height);
        std::memcpy(destination, bgra, buffer.Length());
        m_pixelHash = widget_bridge::PixelHash(reinterpret_cast<std::uint32_t const*>(destination),
                                               static_cast<std::size_t>(width) * height);
        if (m_pixelHash != sourceHash) return HRESULT_FROM_WIN32(ERROR_CRC);
        for (auto& module : m_modules) {
            if (!module.bitmap) return HRESULT_FROM_WIN32(ERROR_NOT_READY);
            const auto moduleWidth = static_cast<std::size_t>(module.bitmapRect.right - module.bitmapRect.left);
            const auto moduleHeight = static_cast<std::size_t>(module.bitmapRect.bottom - module.bitmapRect.top);
            auto moduleBuffer = module.bitmap.PixelBuffer();
            if (moduleBuffer.Length() != moduleWidth * moduleHeight * sizeof(std::uint32_t))
                return HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
            auto moduleAccess = moduleBuffer.as<::Windows::Storage::Streams::IBufferByteAccess>();
            byte* moduleDestination = nullptr;
            check_hresult(moduleAccess->Buffer(&moduleDestination));
            for (std::size_t y = 0; y < moduleHeight; ++y) {
                const auto* source = bgra + (static_cast<std::size_t>(module.bitmapRect.top) + y) * width +
                                     static_cast<std::size_t>(module.bitmapRect.left);
                std::memcpy(moduleDestination + y * moduleWidth * sizeof(std::uint32_t),
                            source, moduleWidth * sizeof(std::uint32_t));
            }
            module.bitmap.Invalidate();
        }
        m_pixelCopyMatches = true;
        m_bitmap.Invalidate();
        ++m_invalidateCount;
        m_frameValid = true;
        m_image.Width(width * 96.0 / dpi);
        m_image.Height(height * 96.0 / dpi);
        m_root.Width(width * 96.0 / dpi);
        m_root.Height(height * 96.0 / dpi);
        LayoutModules();
        if (!hadValidFrame || resize || scaleChanged || moduleLayoutChanged) {
            m_presentation.RequireFreshRender();
            if (!moduleLayoutChanged) EnsureRenderingObservation(); // the layout switch attaches its new roots before visibility is requested
        }
        return S_OK;
    } catch (hresult_error const& e) {
        return e.code();
    } catch (...) {
        return E_FAIL;
    }
}

HRESULT WidgetSurfaceView::GetScreenRect(HWND taskbarWindow, RECT& screenRect) noexcept {
    m_taskbarWindow = taskbarWindow;
    m_hostGateChecked = true;
    m_hostGateReady = false;
    m_taskbarWindowVisible = taskbarWindow && IsWindow(taskbarWindow) && IsWindowVisible(taskbarWindow);
    m_surfaceUncovered = false;
    m_occludingWindow = 0;
    m_occludingPid = 0;
    m_taskbarVisibleSampleCount = 0;
    m_hostGateFailure = L"not-checked";
    if (!m_root || !taskbarWindow || !IsWindow(taskbarWindow) || !m_width || !m_height || !m_dpi)
        return E_INVALIDARG;
    const auto diagnostic = Inspect();
    if (!diagnostic.surfaceReady) return HRESULT_FROM_WIN32(ERROR_NOT_READY);
    if (!diagnostic.xamlRootHostVisible) {
        m_haveHostVisibility = true;
        m_lastHostVisible = false;
        m_hostGateFailure = L"xaml-host-not-visible";
        return HRESULT_FROM_WIN32(ERROR_NOT_READY);
    }
    if (!m_taskbarWindowVisible) {
        m_haveHostVisibility = true;
        m_lastHostVisible = false;
        m_hostGateFailure = L"taskbar-window-hidden";
        return HRESULT_FROM_WIN32(ERROR_NOT_READY);
    }
    if (m_haveHostVisibility && !m_lastHostVisible) {
        m_lastHostVisible = true;
        m_presentation.RequireFreshRender();
        EnsureRenderingObservation();
        m_hostGateFailure = L"host-restored-wait-render";
        return HRESULT_FROM_WIN32(ERROR_NOT_READY);
    }
    m_haveHostVisibility = true;
    m_lastHostVisible = true;
    if (!diagnostic.presentationReady) {
        m_hostGateFailure = L"presentation-pending";
        return HRESULT_FROM_WIN32(ERROR_NOT_READY);
    }
    try {
        if (m_individual) {
            RECT united{};
            bool first = true;
            for (auto const& module : m_modules) {
                auto transform = module.root.TransformToVisual(nullptr);
                auto point = transform.TransformPoint(winrt::Windows::Foundation::Point{0, 0});
                const double scale = static_cast<double>(m_dpi) / 96.0;
                POINT origin{0, 0};
                SetLastError(ERROR_SUCCESS);
                if (!MapWindowPoints(taskbarWindow, nullptr, &origin, 1) && GetLastError() != ERROR_SUCCESS)
                    return HRESULT_FROM_WIN32(GetLastError());
                RECT rect{origin.x + static_cast<LONG>(std::lround(point.X * scale)),
                          origin.y + static_cast<LONG>(std::lround(point.Y * scale)),
                          origin.x + static_cast<LONG>(std::lround(point.X * scale)) +
                              static_cast<LONG>(std::lround(module.root.ActualWidth() * scale)),
                          origin.y + static_cast<LONG>(std::lround(point.Y * scale)) +
                              static_cast<LONG>(std::lround(module.root.ActualHeight() * scale))};
                if (rect.right <= rect.left || rect.bottom <= rect.top) return HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
                if (first) { united = rect; first = false; }
                else { united.left = (std::min)(united.left, rect.left); united.top = (std::min)(united.top, rect.top);
                       united.right = (std::max)(united.right, rect.right); united.bottom = (std::max)(united.bottom, rect.bottom); }
            }
            if (first) return HRESULT_FROM_WIN32(ERROR_NOT_READY);
            RECT bar{};
            if (!GetWindowRect(taskbarWindow, &bar) || united.left < bar.left || united.top < bar.top ||
                united.right > bar.right || united.bottom > bar.bottom) return HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
            HWND occluder = nullptr;
            m_surfaceUncovered = !SampleCoveredByWindow(taskbarWindow, united, occluder, m_taskbarVisibleSampleCount);
            if (!m_surfaceUncovered && !m_hostPresentationCommitted) return HRESULT_FROM_WIN32(ERROR_NOT_READY);
            m_hostPresentationCommitted = true;
            m_hostGateReady = true;
            m_hostGateFailure = L"individual-modules-visible";
            screenRect = united;
            return S_OK;
        }
        auto transform = m_root.TransformToVisual(nullptr);
        auto point = transform.TransformPoint(winrt::Windows::Foundation::Point{ 0, 0 });
        const double scale = static_cast<double>(m_dpi) / 96.0;
        POINT origin{ 0, 0 };
        SetLastError(ERROR_SUCCESS);
        if (!MapWindowPoints(taskbarWindow, nullptr, &origin, 1)) {
            // MapWindowPoints legitimately returns zero when the origin remains zero.
            DWORD const error = GetLastError();
            if (error != ERROR_SUCCESS) return HRESULT_FROM_WIN32(error);
        }
        RECT rect{
            origin.x + static_cast<LONG>(std::lround(point.X * scale)),
            origin.y + static_cast<LONG>(std::lround(point.Y * scale)),
            origin.x + static_cast<LONG>(std::lround(point.X * scale)) + static_cast<LONG>(m_width),
            origin.y + static_cast<LONG>(std::lround(point.Y * scale)) + static_cast<LONG>(m_height)
        };
        RECT bar{};
        if (!GetWindowRect(taskbarWindow, &bar) || rect.left < bar.left || rect.top < bar.top ||
            rect.right > bar.right || rect.bottom > bar.bottom || rect.right <= rect.left || rect.bottom <= rect.top) {
            m_hostGateFailure = L"widget-outside-taskbar";
            return HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
        }
        HWND occluder = nullptr;
        m_surfaceUncovered = !SampleCoveredByWindow(taskbarWindow, rect, occluder,
                                                    m_taskbarVisibleSampleCount);
        if (!m_surfaceUncovered) {
            m_occludingWindow = reinterpret_cast<std::uintptr_t>(occluder);
            if (occluder) {
                DWORD pid = 0;
                GetWindowThreadProcessId(occluder, &pid);
                m_occludingPid = pid;
            }
            if (!HostCanPresent(diagnostic.xamlRootHostVisible, m_taskbarWindowVisible,
                                m_surfaceUncovered, m_hostPresentationCommitted)) {
                m_hostGateFailure = L"surface-covered";
                return HRESULT_FROM_WIN32(ERROR_NOT_READY);
            }
            m_hostGateFailure = L"surface-covered-after-commit";
        } else {
            m_hostGateFailure = m_hostPresentationCommitted ? L"host-visible-committed" : L"host-visible-uncovered";
            if (HostCanPresent(diagnostic.xamlRootHostVisible, m_taskbarWindowVisible,
                               m_surfaceUncovered, m_hostPresentationCommitted))
                m_hostPresentationCommitted = true;
        }
        m_hostGateReady = true;
        screenRect = rect;
        return S_OK;
    } catch (winrt::hresult_error const& e) {
        return e.code();
    } catch (...) {
        return E_FAIL;
    }
}

HRESULT WidgetSurfaceView::SetSurfaceVisible(bool visible) noexcept {
    try {
        if (!m_root) return E_UNEXPECTED;
        const bool changed = m_presentation.SetVisible(visible);
        if (changed) m_root.Opacity(visible ? 1.0 : 0.0);
        for (auto& module : m_modules) if (changed) module.root.Opacity(visible ? 1.0 : 0.0);
        if (visible) EnsureRenderingObservation();
        else StopRenderingObservation();
        if (!visible && (m_dragging || m_dragArmed)) {
            m_dragging = false;
            m_dragArmed = false;
            m_pointerCaptureSucceeded = false;
            m_pressedKind = 0;
            if (m_cancelDrag) m_cancelDrag();
            try { m_root.ReleasePointerCaptures(); } catch (...) { }
        }
        if (!visible) for (auto& module : m_modules) {
            if (module.dragging && m_cancelModuleDrag) m_cancelModuleDrag(module.index);
            module.captured = module.dragging = false;
            try { module.root.ReleasePointerCaptures(); } catch (...) { }
        }
        if (!visible && changed && m_hoveredItem != -2) {
            m_hoveredItem = -2;
            m_pointerInside = false;
            m_pressedKind = 0;
            Send(InputEvent::Leave, -1);
        }
        return S_OK;
    }
    catch (hresult_error const& e) { return e.code(); }
    catch (...) { return E_FAIL; }
}

void WidgetSurfaceView::EnsureRenderingObservation() noexcept {
    if (!m_presentation.NeedsObservation() || m_renderingSubscribed) return;
    try {
        m_renderSubscribedAt = GetTickCount64();
        m_renderingToken = CompositionTarget::Rendering([this](auto const&, auto const&) {
            ++m_renderCallbackCount;
            m_renderCallbackThreadId = GetCurrentThreadId();
            if (m_renderCallbackThreadId != m_dispatcherThreadId) {
                m_renderObserverFailed = true;
                m_hostGateFailure = L"render-callback-wrong-thread";
                return;
            }
            const auto diagnostic = Inspect();
            if (m_presentation.OnRenderPass(diagnostic.layoutReady, diagnostic.xamlRootHostVisible)) {
                ++m_renderPassCount;
                m_renderLatencyMs = GetTickCount64() - m_renderSubscribedAt;
            }
            StopRenderingObservation();
        });
        m_renderingSubscribed = true;
        ++m_renderSubscriptionCount;
        m_renderObserverFailed = false;
    } catch (...) {
        m_renderObserverFailed = true;
        m_hostGateFailure = L"render-subscribe-failed";
    }
}

void WidgetSurfaceView::StopRenderingObservation() noexcept {
    if (!m_renderingSubscribed) return;
    try {
        CompositionTarget::Rendering(m_renderingToken);
        m_renderingSubscribed = false;
        ++m_renderUnsubscribeCount;
    } catch (...) {
        m_renderObserverFailed = true;
        m_hostGateFailure = L"render-unsubscribe-failed";
    }
}

SurfaceDiagnostics WidgetSurfaceView::Inspect() const noexcept {
    SurfaceDiagnostics out{};
    out.sessionId = m_sessionId;
    out.generation = m_generation;
    out.widthPx = m_width;
    out.heightPx = m_height;
    out.dpi = m_dpi;
    out.invalidateCount = m_invalidateCount;
    out.pixelHash = m_pixelHash;
    out.frameValid = m_frameValid;
    out.pixelCopyMatches = m_pixelCopyMatches;
    out.renderedAfterVisible = m_presentation.presented;
    out.surfaceVisibleRequested = m_presentation.visible;
    out.dispatcherThreadId = m_dispatcherThreadId;
    out.inspectThreadId = GetCurrentThreadId();
    out.dispatcherThreadMatches = out.dispatcherThreadId == out.inspectThreadId;
    out.renderPassCount = m_renderPassCount;
    out.renderCallbackThreadId = m_renderCallbackThreadId;
    out.renderCallbackThreadMatches = !m_renderCallbackThreadId ||
        m_renderCallbackThreadId == m_dispatcherThreadId;
    out.renderSubscriptionCount = m_renderSubscriptionCount;
    out.renderCallbackCount = m_renderCallbackCount;
    out.renderUnsubscribeCount = m_renderUnsubscribeCount;
    out.renderingSubscribed = m_renderingSubscribed;
    out.renderObserverFailed = m_renderObserverFailed;
    out.presentationPending = m_presentation.pending;
    out.renderLatencyMs = m_renderLatencyMs;
    out.rootLoadedEventCount = m_rootLoadedEventCount;
    out.imageLoadedEventCount = m_imageLoadedEventCount;
    out.rootUnloadedEventCount = m_rootUnloadedEventCount;
    out.imageUnloadedEventCount = m_imageUnloadedEventCount;
    out.rootSizeChangedCount = m_rootSizeChangedCount;
    out.imageSizeChangedCount = m_imageSizeChangedCount;
    out.layoutUpdatedCount = m_layoutUpdatedCount;
    out.hostGateChecked = m_hostGateChecked;
    out.hostGateReady = m_hostGateReady;
    out.taskbarWindowVisible = m_taskbarWindowVisible;
    out.surfaceUncovered = m_surfaceUncovered;
    out.taskbarVisibleSampleCount = m_taskbarVisibleSampleCount;
    out.occludingWindow = m_occludingWindow;
    out.occludingPid = m_occludingPid;
    out.hostPresentationCommitted = m_hostPresentationCommitted;
    out.hostGateFailure = m_hostGateFailure;
    try {
        if (!m_root || !m_image) return out;
        auto dispatcher = m_root.Dispatcher();
        out.dispatcherHasThreadAccess = dispatcher && dispatcher.HasThreadAccess();
        auto source = m_image.Source();
        out.imageSourceValid = static_cast<bool>(source);
        out.imageSourceInterfacePointerEqual = source && m_bitmap && get_abi(source) == get_abi(m_bitmap);
        // Different WinRT interface pointers are not COM identity; compare canonical IUnknown.
        out.imageSourceMatchesBitmap = source && m_bitmap && SameComIdentity(source, m_bitmap);
        out.rootLoaded = m_root.IsLoaded();
        out.imageLoaded = m_image.IsLoaded();
        out.rootVisible = m_root.Visibility() == Visibility::Visible;
        out.imageVisible = m_image.Visibility() == Visibility::Visible;
        out.hitTestVisible = m_root.IsHitTestVisible();
        out.rootClipped = static_cast<bool>(m_root.Clip());
        out.imageClipped = static_cast<bool>(m_image.Clip());
        out.rootWidthDip = m_root.Width();
        out.rootHeightDip = m_root.Height();
        out.rootActualWidth = m_root.ActualWidth();
        out.rootActualHeight = m_root.ActualHeight();
        out.imageWidthDip = m_image.Width();
        out.imageHeightDip = m_image.Height();
        out.imageActualWidth = m_image.ActualWidth();
        out.imageActualHeight = m_image.ActualHeight();
        out.rootOpacity = m_root.Opacity();
        out.imageOpacity = m_image.Opacity();
        auto xamlRoot = m_root.XamlRoot();
        out.xamlRootValid = static_cast<bool>(xamlRoot);
        if (xamlRoot) {
            out.xamlRootHostVisible = xamlRoot.IsHostVisible();
            const auto rootSize = xamlRoot.Size();
            out.xamlRootWidth = rootSize.Width;
            out.xamlRootHeight = rootSize.Height;
            out.rasterizationScale = xamlRoot.RasterizationScale();
        }
    } catch (...) { }
    const double expectedWidth = out.dpi ? static_cast<double>(out.widthPx) * 96.0 / out.dpi : 0.0;
    const double expectedHeight = out.dpi ? static_cast<double>(out.heightPx) * 96.0 / out.dpi : 0.0;
    constexpr double tolerance = 1.0;
    out.dimensionsMatch = out.widthPx && out.heightPx && out.dpi &&
        std::abs(out.rootActualWidth - expectedWidth) < tolerance &&
        std::abs(out.rootActualHeight - expectedHeight) < tolerance &&
        std::abs(out.imageActualWidth - expectedWidth) < tolerance &&
        std::abs(out.imageActualHeight - expectedHeight) < tolerance;
    out.layoutReady = out.rootLoaded && out.imageLoaded && out.xamlRootValid && out.dimensionsMatch &&
        !out.rootClipped && !out.imageClipped;
    if (m_individual) {
        out.rootLoaded = !m_modules.empty();
        out.imageLoaded = !m_modules.empty();
        out.xamlRootValid = !m_modules.empty();
        out.xamlRootHostVisible = !m_modules.empty();
        out.dimensionsMatch = !m_modules.empty();
        out.rootClipped = false;
        out.imageClipped = false;
        try {
            out.rootVisible = out.imageVisible = out.hitTestVisible = true;
            out.rootOpacity = out.imageOpacity = 1.0;
            out.rootClipped = out.imageClipped = false;
            out.imageSourceValid = out.imageSourceMatchesBitmap = true;
            for (auto const& module : m_modules) {
                const auto xamlRoot = module.root.XamlRoot();
                const double expectedWidth = static_cast<double>(module.rect.right - module.rect.left) * 96.0 / out.dpi;
                const double expectedHeight = static_cast<double>(module.rect.bottom - module.rect.top) * 96.0 / out.dpi;
                const auto origin = module.root.TransformToVisual(nullptr).TransformPoint({0, 0});
                out.rootLoaded = out.rootLoaded && module.root.IsLoaded();
                out.imageLoaded = out.imageLoaded && module.image.IsLoaded();
                out.rootVisible = out.rootVisible && module.root.Visibility() == Visibility::Visible;
                out.imageVisible = out.imageVisible && module.image.Visibility() == Visibility::Visible;
                const auto moduleSource = module.image.Source();
                out.imageSourceValid = out.imageSourceValid && static_cast<bool>(moduleSource);
                out.imageSourceMatchesBitmap = out.imageSourceMatchesBitmap && moduleSource && module.bitmap &&
                    SameComIdentity(moduleSource, module.bitmap);
                out.hitTestVisible = out.hitTestVisible && module.root.IsHitTestVisible();
                out.rootOpacity = (std::min)(out.rootOpacity, static_cast<double>(module.root.Opacity()));
                out.imageOpacity = (std::min)(out.imageOpacity, static_cast<double>(module.image.Opacity()));
                const auto moduleClip = module.root.Clip().try_as<RectangleGeometry>();
                const auto clipRect = moduleClip ? moduleClip.Rect() : winrt::Windows::Foundation::Rect{};
                out.rootClipped = out.rootClipped || !moduleClip ||
                    std::abs(clipRect.Width - expectedWidth) >= tolerance ||
                    std::abs(clipRect.Height - expectedHeight) >= tolerance;
                out.imageClipped = out.imageClipped || static_cast<bool>(module.image.Clip());
                out.xamlRootValid = out.xamlRootValid && static_cast<bool>(xamlRoot);
                out.xamlRootHostVisible = out.xamlRootHostVisible && xamlRoot && xamlRoot.IsHostVisible();
                out.dimensionsMatch = out.dimensionsMatch &&
                    std::abs(module.root.ActualWidth() - expectedWidth) < tolerance &&
                    std::abs(module.root.ActualHeight() - expectedHeight) < tolerance &&
                    std::abs(module.image.ActualWidth() - expectedWidth) < tolerance &&
                    std::abs(module.image.ActualHeight() - expectedHeight) < tolerance;
            }
        } catch (...) { out.layoutReady = false; }
        out.layoutReady = out.rootLoaded && out.imageLoaded && out.xamlRootValid &&
                          out.xamlRootHostVisible && out.dimensionsMatch;
    }
    out.presentationReady = out.surfaceVisibleRequested && out.renderedAfterVisible;
    if (!out.frameValid) out.firstFailure = L"frame-invalid-or-not-copied";
    else if (!out.widthPx || !out.heightPx || !out.dpi) out.firstFailure = L"frame-dimensions-dpi";
    else if (!out.pixelCopyMatches) out.firstFailure = L"pixel-copy-mismatch";
    else if (!out.imageSourceValid) out.firstFailure = L"image-source-null";
    else if (!out.imageSourceMatchesBitmap) out.firstFailure = L"image-source-bitmap-mismatch";
    else if (!out.dispatcherThreadMatches || !out.dispatcherHasThreadAccess) out.firstFailure = L"xaml-dispatcher-thread";
    else if (!out.invalidateCount) out.firstFailure = L"bitmap-not-invalidated";
    else if (!out.xamlRootValid) out.firstFailure = L"xamlroot-unavailable";
    else if (!out.rootLoaded) out.firstFailure = L"root-not-loaded";
    else if (!out.imageLoaded) out.firstFailure = L"image-not-loaded";
    else if (!out.surfaceVisibleRequested) out.firstFailure = L"surface-not-requested-visible";
    else if (!out.rootVisible || !out.imageVisible) out.firstFailure = L"xaml-visibility";
    else if (!out.hitTestVisible) out.firstFailure = L"hit-test-disabled";
    else if (out.rootClipped || out.imageClipped) out.firstFailure = L"xaml-clipped";
    else if (!out.dimensionsMatch) out.firstFailure = L"actual-size-dip-mismatch";
    else if (out.rootOpacity < 0.99 || out.imageOpacity < 0.99) out.firstFailure = L"xaml-opacity";
    else {
        out.surfaceReady = true;
        if (!out.presentationReady) {
            if (out.renderObserverFailed)
                out.firstFailure = out.hostGateFailure;
            else out.firstFailure = L"presentation-event-pending";
        }
        else if (out.hostGateChecked && !out.hostGateReady)
            out.firstFailure = out.hostGateFailure;
        else out.firstFailure = L"none";
    }
    return out;
}

bool WidgetSurfaceView::IsSurfaceReady() const noexcept {
    return Inspect().surfaceReady;
}

} // namespace batteryhub::xaml
