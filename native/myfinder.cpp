#include <windows.h>
#include <atomic>
#include <memory>
#include <thread>
#include "composition_surface.h"
#include "finder_order_guard.h"
#include "myfinder_layout.h"

namespace {
struct Sample {
    std::atomic<bool> stop{false};
    std::atomic<int> edge{-1};
    std::atomic<int> foreground{0};
    HWND host;
    explicit Sample(HWND window) : host(window) {}
};
struct Field {
    HWND window, host;
    CompositionSurface surface;
    FinderOrderGuard order;
    std::shared_ptr<Sample> sample;
    HWND fullscreen = nullptr;
    ULONGLONG fullscreenSince = 0;
    bool behind = false;
};
HWND ForegroundCoveringFinderMonitor(HWND host) {
    HWND foreground = GetForegroundWindow();
    if (!foreground || foreground == GetDesktopWindow() || foreground == GetShellWindow() ||
        !IsWindowVisible(foreground) || IsIconic(foreground)) return nullptr;
    // Explorer can focus a desktop host whose client rectangle fills a monitor.
    wchar_t windowClass[64]{};
    GetClassNameW(foreground, windowClass, 64);
    if (wcscmp(windowClass, L"Progman") == 0 || wcscmp(windowClass, L"WorkerW") == 0 ||
        wcscmp(windowClass, L"Shell_TrayWnd") == 0 || wcscmp(windowClass, L"Shell_SecondaryTrayWnd") == 0)
        return nullptr;
    DWORD process = 0, finderProcess = 0;
    GetWindowThreadProcessId(foreground, &process);
    GetWindowThreadProcessId(host, &finderProcess);
    if (!process || process == GetCurrentProcessId() || process == finderProcess) return nullptr;
    HMONITOR monitor = MonitorFromWindow(host, MONITOR_DEFAULTTONULL);
    if (!monitor || MonitorFromWindow(foreground, MONITOR_DEFAULTTONULL) != monitor) return nullptr;
    MONITORINFO info{sizeof(info)};
    RECT client{};
    if (!GetMonitorInfoW(monitor, &info) || !GetClientRect(foreground, &client)) return nullptr;
    POINT first{client.left, client.top}, last{client.right, client.bottom};
    if (!ClientToScreen(foreground, &first) || !ClientToScreen(foreground, &last)) return nullptr;
    const RECT& screen = info.rcMonitor;
    return first.x <= screen.left && first.y <= screen.top &&
           last.x >= screen.right && last.y >= screen.bottom ? foreground : nullptr;
}

void ReadLayout(const std::shared_ptr<Sample>& sample) {
    SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    while (!sample->stop.load() && IsWindow(sample->host)) {
        RECT bounds{};
        if (IsWindowVisible(sample->host) && GetWindowRect(sample->host, &bounds)) {
            const int w = bounds.right - bounds.left, h = bounds.bottom - bounds.top;
            if (w >= 8 && w <= 16384 && h >= 4 && h <= 256) {
                HDC dc = CreateCompatibleDC(nullptr);
                BITMAPINFO info{};
                info.bmiHeader = {sizeof(BITMAPINFOHEADER), w, -h, 1, 32, BI_RGB};
                void* bits = nullptr;
                HBITMAP bitmap = dc ? CreateDIBSection(dc, &info, DIB_RGB_COLORS, &bits, nullptr, 0) : nullptr;
                if (bitmap) {
                    auto old = SelectObject(dc, bitmap);
                    if (PrintWindow(sample->host, dc, 2)) {
                        GdiFlush();
                        auto pixels = static_cast<const std::uint32_t*>(bits);
                        int edge = myfinder::GroupLeft(pixels, w, h, std::max(24, h * 2));
                        if (edge >= 0) {
                            sample->edge.store(edge);
                            auto fg = myfinder::GlyphForeground(pixels, w, h, edge);
                            if (fg != myfinder::Foreground::Unknown) sample->foreground.store(static_cast<int>(fg));
                        }
                    }
                    SelectObject(dc, old);
                    DeleteObject(bitmap);
                }
                if (dc) DeleteDC(dc);
            }
        }
        Sleep(125);
    }
}
}
#define API extern "C" __declspec(dllexport)
API Field* __cdecl OpenFinderField(HWND window, HWND host) {
    try {
        auto field = std::make_unique<Field>();
        field->window = window;
        field->host = host;
        if (!field->surface.Initialize(window)) return nullptr;
        field->sample = std::make_shared<Sample>(host);
        std::thread(ReadLayout, field->sample).detach();
        field->order.Start(window, host);
        return field.release();
    } catch (...) { return nullptr; }
}
API void __cdecl CloseFinderField(Field* field) {
    if (!field) return;
    field->sample->stop.store(true);
    delete field;
}
API int __cdecl PaintFinderField(Field* field, const void* pixels, UINT w, UINT h) {
    return field->surface.Paint(pixels, w, h, w * 4) ||
           (field->surface.Initialize(field->window) && field->surface.Paint(pixels, w, h, w * 4));
}
API int __cdecl FinderFieldForeground(Field* field) { return field->sample->foreground.load(); }
API void __cdecl PlaceFinderField(Field* field, int w, int h, int gap) {
    const HWND fullscreen = ForegroundCoveringFinderMonitor(field->host);
    const auto now = GetTickCount64();
    if (fullscreen != field->fullscreen) {
        field->fullscreen = fullscreen;
        field->fullscreenSince = now;
    }
    if (fullscreen && (field->behind || now - field->fullscreenSince >= 350)) {
        if (!field->behind) {
            field->behind = true;
            field->order.Stop();
            SetWindowPos(field->window, HWND_BOTTOM, 0, 0, 0, 0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
        return;
    }
    if (field->behind) {
        field->behind = false;
        field->order.Start(field->window, field->host);
    }
    RECT bar{}, current{};
    const int edge = field->sample->edge.load();
    if (!IsWindowVisible(field->host) || IsIconic(field->host) || !GetWindowRect(field->host, &bar)) {
        if (IsWindowVisible(field->window)) ShowWindow(field->window, SW_HIDE);
        return;
    }
    if (w <= 0 || h <= 0 || edge < w + gap || edge >= bar.right - bar.left) return;
    const int x = bar.left + edge - w - gap;
    const bool top = (GetWindowLongPtrW(field->host, GWL_EXSTYLE) & WS_EX_TOPMOST) != 0;
    const bool ours = (GetWindowLongPtrW(field->window, GWL_EXSTYLE) & WS_EX_TOPMOST) != 0;
    constexpr UINT flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    if (top != ours) SetWindowPos(field->window, top ? HWND_TOPMOST : HWND_NOTOPMOST, 0, 0, 0, 0, flags | SWP_NOMOVE | SWP_NOSIZE);
    if (GetWindowRect(field->window, &current) && IsWindowVisible(field->window) &&
        current.left == x && current.top == bar.top && current.right - x == w && current.bottom - bar.top == h) return;
    SetWindowPos(field->window, nullptr, x, bar.top, w, h, flags | SWP_NOZORDER | SWP_SHOWWINDOW);
}
