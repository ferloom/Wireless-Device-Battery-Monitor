#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#undef GetCurrentTime
#include <shellapi.h>
#include <shlwapi.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Media.h>

#include "TaskbarSlot.h"
#include "TaskbarXamlModule.h"
#include "../taskbar-xaml-widget/WidgetSurfaceView.h"

#include <memory>
#include <string>
#include <vector>

using namespace winrt;
using namespace winrt::Windows::Foundation;
using namespace winrt::Windows::UI::Xaml;
using namespace winrt::Windows::UI::Xaml::Controls;

namespace {
constexpr unsigned int kFailureLimit = 3;
constexpr wchar_t kTrayName[] = L"SystemTrayFrameGrid";
constexpr wchar_t kAnchorName[] = L"NotifyIconStack";
constexpr size_t kMaxTreeNodes = 4096;

TaskbarSlot g_slot;
struct ModuleSlot {
    std::uint64_t id = 0;
    std::uint32_t item = 0;
    int anchor = -1;
    std::unique_ptr<TaskbarSlot> slot;
};
std::vector<ModuleSlot> g_moduleSlots;
std::vector<std::uint64_t> g_layoutIds;
std::vector<std::int32_t> g_layoutAnchors;
bool g_individual = false;
FrameworkElement g_widget{ nullptr };
std::unique_ptr<batteryhub::xaml::WidgetSurfaceView> g_surfaceView;
IInspectable g_trayObject{ nullptr };
DWORD g_taskbarThread = 0;
unsigned int g_failures = 0;
bool g_disabled = false;
std::wstring g_logPath;

bool RemoveAllSlots() noexcept;

void Log(wchar_t const* eventName, HRESULT hr = S_OK) noexcept {
    SYSTEMTIME now{};
    GetSystemTime(&now);
    wchar_t line[384]{};
    swprintf_s(line, L"%04u-%02u-%02uT%02u:%02u:%02u.%03uZ event=%s hr=0x%08X pid=%lu tid=%lu taskbarTid=%lu failures=%u disabled=%u\r\n",
               now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond,
               now.wMilliseconds, eventName, static_cast<unsigned int>(hr), GetCurrentProcessId(),
               GetCurrentThreadId(), g_taskbarThread, g_failures, g_disabled ? 1u : 0u);
    OutputDebugStringW(line);
    if (g_logPath.empty()) return;
    HANDLE file = CreateFileW(g_logPath.c_str(), FILE_APPEND_DATA,
                              FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                              nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return;
    char utf8[sizeof(line) * 4]{};
    const int encodedBytes = WideCharToMultiByte(CP_UTF8, 0, line, -1, utf8,
                                                  static_cast<int>(sizeof(utf8)), nullptr, nullptr);
    DWORD written = 0;
    if (encodedBytes > 1) WriteFile(file, utf8, static_cast<DWORD>(encodedBytes - 1), &written, nullptr);
    CloseHandle(file);
}

void LogSurface(wchar_t const* stage, HRESULT hr, RECT const* rect = nullptr) noexcept {
    if (!g_surfaceView || g_logPath.empty()) return;
    const auto d = g_surfaceView->Inspect();
    SYSTEMTIME now{};
    GetSystemTime(&now);
    wchar_t line[2600]{};
    const int chars = swprintf_s(line,
        L"{\"tsUtc\":\"%04u-%02u-%02uT%02u:%02u:%02u.%03uZ\",\"pid\":%lu,\"tid\":%lu,\"taskbarTid\":%lu,"
        L"\"session\":\"%016llX\",\"frame\":%lu,\"stage\":\"%s\",\"hr\":\"0x%08X\","
        L"\"readiness\":\"%s\",\"surfaceReady\":%s,\"layoutReady\":%s,\"presentationReady\":%s,"
        L"\"presentationPending\":%s,\"firstFailure\":\"%s\",\"hostReady\":%s,"
        L"\"widthPx\":%lu,\"heightPx\":%lu,\"dpi\":%lu,\"stride\":%lu,\"pixelBytes\":%llu,"
        L"\"pixelHash\":\"%016llX\",\"copyMatches\":%s,\"invalidateCount\":%lu,"
        L"\"imageSource\":%s,\"sourceInterfacePointerEqual\":%s,\"sourceMatchesBitmap\":%s,"
        L"\"rootLoaded\":%s,\"imageLoaded\":%s,\"xamlRootValid\":%s,\"xamlRootHostVisible\":%s,"
        L"\"xamlRootSizeDip\":[%.2f,%.2f],\"rasterizationScale\":%.3f,"
        L"\"rootVisible\":%s,\"imageVisible\":%s,\"rootOpacity\":%.3f,\"imageOpacity\":%.3f,"
        L"\"rootWidthDip\":%.2f,\"rootHeightDip\":%.2f,\"rootActualWidth\":%.2f,"
        L"\"rootActualHeight\":%.2f,\"imageActualWidth\":%.2f,\"imageActualHeight\":%.2f,"
        L"\"hitTestVisible\":%s,\"rootClipped\":%s,\"imageClipped\":%s,\"rendered\":%s,"
        L"\"visibleRequested\":%s,\"renderingSubscribed\":%s,\"renderObserverFailed\":%s,\"renderPassCount\":%lu,"
        L"\"renderSubscriptions\":%lu,\"renderCallbacks\":%lu,\"renderUnsubscribes\":%lu,\"renderLatencyMs\":%llu,"
        L"\"renderCallbackThread\":%lu,\"renderCallbackThreadMatches\":%s,"
        L"\"loadedEvents\":[%lu,%lu],\"unloadedEvents\":[%lu,%lu],"
        L"\"sizeChangedEvents\":[%lu,%lu],\"layoutUpdatedEvents\":%lu,"
        L"\"dispatcherThread\":%lu,\"inspectThread\":%lu,\"dispatcherHasAccess\":%s,\"dispatcherMatches\":%s,"
        L"\"dimensionsMatch\":%s,\"taskbarVisible\":%s,\"hostGateReady\":%s,"
        L"\"surfaceUncovered\":%s,\"uncoveredSamples\":%lu,"
        L"\"hostPresentationCommitted\":%s,\"occludingWindow\":\"0x%llX\","
        L"\"occludingPid\":%lu,\"hostGateFailure\":\"%s\"",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds,
        GetCurrentProcessId(), GetCurrentThreadId(), g_taskbarThread,
        static_cast<unsigned long long>(d.sessionId), d.generation,
        stage, static_cast<unsigned>(hr),
        d.surfaceReady ? L"SurfaceReady" : (d.frameValid ? L"FrameValid" : L"WaitingForFrame"),
        d.surfaceReady ? L"true" : L"false", d.layoutReady ? L"true" : L"false",
        d.presentationReady ? L"true" : L"false", d.presentationPending ? L"true" : L"false", d.firstFailure,
        SUCCEEDED(hr) && rect ? L"true" : L"false",
        d.widthPx, d.heightPx, d.dpi, d.widthPx * 4u,
        static_cast<unsigned long long>(static_cast<std::uint64_t>(d.widthPx) * d.heightPx * 4u),
        static_cast<unsigned long long>(d.pixelHash), d.pixelCopyMatches ? L"true" : L"false", d.invalidateCount,
        d.imageSourceValid ? L"true" : L"false", d.imageSourceInterfacePointerEqual ? L"true" : L"false",
        d.imageSourceMatchesBitmap ? L"true" : L"false",
        d.rootLoaded ? L"true" : L"false", d.imageLoaded ? L"true" : L"false",
        d.xamlRootValid ? L"true" : L"false", d.xamlRootHostVisible ? L"true" : L"false",
        d.xamlRootWidth, d.xamlRootHeight, d.rasterizationScale,
        d.rootVisible ? L"true" : L"false", d.imageVisible ? L"true" : L"false",
        d.rootOpacity, d.imageOpacity, d.rootWidthDip, d.rootHeightDip, d.rootActualWidth, d.rootActualHeight,
        d.imageActualWidth, d.imageActualHeight, d.hitTestVisible ? L"true" : L"false",
        d.rootClipped ? L"true" : L"false", d.imageClipped ? L"true" : L"false",
        d.renderedAfterVisible ? L"true" : L"false", d.surfaceVisibleRequested ? L"true" : L"false",
        d.renderingSubscribed ? L"true" : L"false", d.renderObserverFailed ? L"true" : L"false",
        d.renderPassCount,
        d.renderSubscriptionCount, d.renderCallbackCount, d.renderUnsubscribeCount,
        static_cast<unsigned long long>(d.renderLatencyMs),
        d.renderCallbackThreadId, d.renderCallbackThreadMatches ? L"true" : L"false",
        d.rootLoadedEventCount, d.imageLoadedEventCount, d.rootUnloadedEventCount, d.imageUnloadedEventCount,
        d.rootSizeChangedCount, d.imageSizeChangedCount,
        d.layoutUpdatedCount,
        d.dispatcherThreadId, d.inspectThreadId,
        d.dispatcherHasThreadAccess ? L"true" : L"false", d.dispatcherThreadMatches ? L"true" : L"false",
        d.dimensionsMatch ? L"true" : L"false", d.taskbarWindowVisible ? L"true" : L"false",
        d.hostGateReady ? L"true" : L"false", d.surfaceUncovered ? L"true" : L"false",
        d.taskbarVisibleSampleCount,
        d.hostPresentationCommitted ? L"true" : L"false",
        static_cast<unsigned long long>(d.occludingWindow), d.occludingPid, d.hostGateFailure);
    if (chars <= 0 || static_cast<std::size_t>(chars) >= ARRAYSIZE(line)) return;
    std::size_t length = static_cast<std::size_t>(chars);
    if (rect) {
        const int appended = swprintf_s(line + length, ARRAYSIZE(line) - length,
            L",\"rect\":[%ld,%ld,%ld,%ld]", rect->left, rect->top, rect->right, rect->bottom);
        if (appended <= 0 || static_cast<std::size_t>(appended) >= ARRAYSIZE(line) - length) return;
        length += static_cast<std::size_t>(appended);
    }
    if (length + 3 >= ARRAYSIZE(line)) return;
    line[length++] = L'}';
    line[length++] = L'\r';
    line[length++] = L'\n';
    char utf8[sizeof(line) * 4]{};
    const int bytes = WideCharToMultiByte(CP_UTF8, 0, line, static_cast<int>(length), utf8,
                                          static_cast<int>(sizeof(utf8)), nullptr, nullptr);
    if (bytes <= 0) return;
    HANDLE file = CreateFileW(g_logPath.c_str(), FILE_APPEND_DATA,
                              FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                              nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return;
    DWORD written = 0;
    WriteFile(file, utf8, static_cast<DWORD>(bytes), &written, nullptr);
    CloseHandle(file);
}

void RecordFailure(wchar_t const* eventName, HRESULT hr) noexcept {
    if (g_failures < kFailureLimit) ++g_failures;
    Log(eventName, hr);
    if (g_failures >= kFailureLimit && !g_disabled) {
        g_disabled = true;
        if (g_taskbarThread == GetCurrentThreadId() && (g_slot.IsAttached() || !g_moduleSlots.empty())) {
            try {
                if (RemoveAllSlots()) {
                    g_trayObject = nullptr;
                    g_widget = nullptr;
                    g_surfaceView.reset();
                }
            } catch (...) { }
        }
        Log(L"circuit-open", hr);
    }
}

HRESULT CheckThread() noexcept {
    const DWORD current = GetCurrentThreadId();
    if (g_taskbarThread == 0) g_taskbarThread = current;
    if (g_taskbarThread == current) return S_OK;
    Log(L"wrong-dispatcher-thread", RPC_E_WRONG_THREAD);
    return RPC_E_WRONG_THREAD;
}

HRESULT AttachFromAbi(void* trayAbi, BOOL primaryHorizontalTaskbar, wchar_t const* operation) noexcept;

FrameworkElement MakePreviewWidget() {
    g_surfaceView = std::make_unique<batteryhub::xaml::WidgetSurfaceView>();
    auto widget = g_surfaceView->Root();
    widget.Name(L"BatteryHubTaskbarPrototype");
    widget.Width(130.0);
    widget.Height(40.0);
    widget.VerticalAlignment(VerticalAlignment::Center);
    widget.HorizontalAlignment(HorizontalAlignment::Center);
    return widget;
}

bool RemoveModuleSlots() noexcept {
    bool ok = true;
    for (auto it = g_moduleSlots.rbegin(); it != g_moduleSlots.rend(); ++it)
        if (it->slot && it->slot->IsAttached()) ok = it->slot->Remove() && ok;
    g_moduleSlots.clear();
    g_layoutIds.clear();
    g_layoutAnchors.clear();
    return ok;
}

bool RemoveAllSlots() noexcept {
    const bool modules = RemoveModuleSlots();
    const bool grouped = !g_slot.IsAttached() || g_slot.Remove();
    g_individual = false;
    return modules && grouped;
}

bool ConfigureGroupedSlot() {
    if (!g_surfaceView || !g_slot.IsAttached()) return false;
    auto root = g_surfaceView->Root();
    g_surfaceView->SetDragCallbacks(
        [root](double x) {
            try { return g_slot.PreviewAt(root.TransformToVisual(g_slot.TrayElement()).TransformPoint({static_cast<float>(x), 0.0f}).X); }
            catch (...) { return -1; }
        },
        [] { return g_slot.CommitPreview(); },
        [] { g_slot.CancelPreview(); },
        [](wchar_t const* event) { Log(event); });
    return true;
}

bool InsertGroupedSlot() {
    if (!g_surfaceView || !g_trayObject) return false;
    auto root = g_surfaceView->Root();
    bool inserted = false;
    if (auto grid = g_trayObject.try_as<Grid>()) inserted = g_slot.InsertAfter(grid, kAnchorName, root);
    else if (auto stack = g_trayObject.try_as<StackPanel>()) inserted = g_slot.InsertAfter(stack, kAnchorName, root);
    return inserted && ConfigureGroupedSlot();
}

bool BuildModuleSlots(std::uint64_t const* ids, std::int32_t const* anchors, std::uint32_t count) {
    if (!g_surfaceView || !g_trayObject || (count && (!ids || !anchors))) return false;
    try {
        g_moduleSlots.reserve(count);
        for (std::uint32_t remaining = count; remaining > 0; --remaining) {
            const std::uint32_t i = remaining - 1;
            ModuleSlot entry{};
            entry.id = ids[i];
            entry.item = i;
            entry.anchor = anchors[i];
            entry.slot = std::make_unique<TaskbarSlot>();
            auto root = g_surfaceView->ModuleRoot(entry.id);
            bool inserted = false;
            if (auto grid = g_trayObject.try_as<Grid>()) inserted = entry.slot->InsertAfter(grid, kAnchorName, root);
            else if (auto stack = g_trayObject.try_as<StackPanel>()) inserted = entry.slot->InsertAfter(stack, kAnchorName, root);
            if (!inserted) { RemoveModuleSlots(); return false; }
            if (!entry.slot->SetDesiredAnchor(entry.anchor)) {
                entry.anchor = -1;
                if (!entry.slot->SetDesiredAnchor(-1)) { entry.slot->Remove(); RemoveModuleSlots(); return false; }
            }
            g_moduleSlots.push_back(std::move(entry));
        }
        if (count) {
            g_layoutIds.assign(ids, ids + count);
            g_layoutAnchors.assign(anchors, anchors + count);
        } else {
            g_layoutIds.clear();
            g_layoutAnchors.clear();
        }
        g_surfaceView->SetModuleDragCallbacks(
            [](std::size_t item, double x) {
                auto it = std::find_if(g_moduleSlots.begin(), g_moduleSlots.end(), [item](auto const& s) { return s.item == item; });
                if (it == g_moduleSlots.end() || !it->slot || !it->slot->TrayElement()) return -1;
                try {
                    auto root = g_surfaceView->ModuleRoot(it->id);
                    const double trayX = root.TransformToVisual(it->slot->TrayElement()).TransformPoint({static_cast<float>(x), 0.0f}).X;
                    return it->slot->PreviewAnchorAt(trayX);
                } catch (...) { return -1; }
            },
            [](std::size_t item) {
                auto it = std::find_if(g_moduleSlots.begin(), g_moduleSlots.end(), [item](auto const& s) { return s.item == item; });
                return it == g_moduleSlots.end() || !it->slot ? -1 : it->slot->CommitAnchorPreview();
            },
            [](std::size_t item) {
                auto it = std::find_if(g_moduleSlots.begin(), g_moduleSlots.end(), [item](auto const& s) { return s.item == item; });
                if (it != g_moduleSlots.end() && it->slot) it->slot->CancelPreview();
            });
        return true;
    } catch (...) { RemoveModuleSlots(); return false; }
}

bool IsSafeHorizontalGrid(Grid const& grid, FrameworkElement const& anchor) {
    if (grid.RowDefinitions().Size() > 1) return false;
    const int anchorRow = Grid::GetRow(anchor);
    const int anchorColumn = Grid::GetColumn(anchor);
    bool hasFollowingColumn = false;
    for (auto const& child : grid.Children()) {
        auto element = child.try_as<FrameworkElement>();
        if (!element) return false;
        if (Grid::GetRow(element) != anchorRow ||
            Grid::GetRowSpan(element) != 1 || Grid::GetColumnSpan(element) != 1) return false;
        if (Grid::GetColumn(element) > anchorColumn) hasFollowingColumn = true;
    }
    return hasFollowingColumn;
}

HRESULT AttachCore(IInspectable const& trayObject) {
    if (g_disabled) return HRESULT_FROM_WIN32(ERROR_SERVICE_DISABLED);
    if (auto grid = trayObject.try_as<Grid>()) {
        if (grid.Name() != kTrayName) return HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
        FrameworkElement anchor{ nullptr };
        for (auto const& child : grid.Children()) {
            auto element = child.try_as<FrameworkElement>();
            if (element && element.Name() == kAnchorName) { anchor = element; break; }
        }
        if (!anchor || !IsSafeHorizontalGrid(grid, anchor)) return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
        if (!g_widget) g_widget = MakePreviewWidget();
        if (!g_slot.InsertAfter(grid, kAnchorName, g_widget)) return HRESULT_FROM_WIN32(ERROR_INVALID_STATE);
        ConfigureGroupedSlot();
        if (g_surfaceView) g_surfaceView->SetSurfaceVisible(false);
        Log(L"drag-layout-grid-attached");
        LogSurface(L"visual-inserted-hidden", S_OK);
        g_trayObject = trayObject;
        return S_OK;
    }
    if (auto stack = trayObject.try_as<StackPanel>()) {
        if (stack.Name() != kTrayName || stack.Orientation() != Orientation::Horizontal)
            return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
        if (!g_widget) g_widget = MakePreviewWidget();
        if (!g_slot.InsertAfter(stack, kAnchorName, g_widget)) return HRESULT_FROM_WIN32(ERROR_NOT_FOUND);
        ConfigureGroupedSlot();
        if (g_surfaceView) g_surfaceView->SetSurfaceVisible(false);
        Log(L"drag-layout-stack-attached");
        LogSurface(L"visual-inserted-hidden", S_OK);
        g_trayObject = trayObject;
        return S_OK;
    }
    return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
}

HRESULT FindTaskbarTray(UIElement const& root, FrameworkElement& result) {
    using namespace winrt::Windows::UI::Xaml::Media;
    if (!root) return E_POINTER;
    std::vector<DependencyObject> pending{ root };
    FrameworkElement found{ nullptr };
    size_t visited = 0;
    while (!pending.empty()) {
        if (++visited > kMaxTreeNodes) return HRESULT_FROM_WIN32(ERROR_BUFFER_OVERFLOW);
        auto node = std::move(pending.back());
        pending.pop_back();
        auto element = node.try_as<FrameworkElement>();
        if (element && element.Name() == kTrayName) {
            if (found) return HRESULT_FROM_WIN32(ERROR_DUP_NAME);
            found = element;
        }
        const auto count = VisualTreeHelper::GetChildrenCount(node);
        if (count > kMaxTreeNodes - pending.size()) return HRESULT_FROM_WIN32(ERROR_BUFFER_OVERFLOW);
        for (int i = static_cast<int>(count) - 1; i >= 0; --i)
            pending.push_back(VisualTreeHelper::GetChild(node, i));
    }
    if (!found) return HRESULT_FROM_WIN32(ERROR_NOT_FOUND);

    unsigned int anchorCount = 0;
    auto countDirectAnchors = [&](auto const& children) {
        for (auto const& child : children) {
            auto childElement = child.try_as<FrameworkElement>();
            if (childElement && childElement.Name() == kAnchorName) ++anchorCount;
        }
    };
    if (auto grid = found.try_as<Grid>()) countDirectAnchors(grid.Children());
    else if (auto stack = found.try_as<StackPanel>()) countDirectAnchors(stack.Children());
    else return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    if (anchorCount != 1) return HRESULT_FROM_WIN32(anchorCount ? ERROR_DUP_NAME : ERROR_NOT_FOUND);
    result = found;
    return S_OK;
}

HRESULT AttachTreeFromAbi(void* rootContentAbi, BOOL primaryHorizontalTaskbar, wchar_t const* operation) noexcept {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    if (g_disabled) return HRESULT_FROM_WIN32(ERROR_SERVICE_DISABLED);
    if (!primaryHorizontalTaskbar) hr = HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    else if (!rootContentAbi) hr = E_POINTER;
    else {
        try {
            IInspectable rootObject{ nullptr };
            copy_from_abi(rootObject, rootContentAbi);
            auto root = rootObject.try_as<UIElement>();
            if (!root) hr = HRESULT_FROM_WIN32(ERROR_INVALID_DATA);
            else {
                FrameworkElement tray{ nullptr };
                hr = FindTaskbarTray(root, tray);
                if (SUCCEEDED(hr)) return AttachFromAbi(get_abi(tray), primaryHorizontalTaskbar, operation);
            }
        } catch (hresult_error const& e) {
            hr = e.code();
        } catch (...) {
            hr = E_FAIL;
        }
    }
    if (FAILED(hr)) RecordFailure(operation, hr);
    return hr;
}

HRESULT VerifyPrimaryHorizontalTaskbar(HWND taskbarWindow) noexcept {
    if (!taskbarWindow || !IsWindow(taskbarWindow)) return E_HANDLE;
    const HWND primaryTaskbar = FindWindowW(L"Shell_TrayWnd", nullptr);
    if (!primaryTaskbar || taskbarWindow != primaryTaskbar)
        return HRESULT_FROM_WIN32(ERROR_NOT_FOUND);

    APPBARDATA appbar{};
    appbar.cbSize = sizeof(appbar);
    if (!SHAppBarMessage(ABM_GETTASKBARPOS, &appbar))
        return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    if (appbar.uEdge != ABE_BOTTOM && appbar.uEdge != ABE_TOP)
        return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);

    RECT windowRect{};
    if (!GetWindowRect(taskbarWindow, &windowRect)) return HRESULT_FROM_WIN32(GetLastError());
    if (windowRect.right - windowRect.left <= windowRect.bottom - windowRect.top ||
        appbar.rc.right - appbar.rc.left <= appbar.rc.bottom - appbar.rc.top)
        return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    return S_OK;
}

HRESULT AttachVerifiedTaskbarTree(void* rootContentAbi, HWND taskbarWindow, wchar_t const* operation) noexcept {
    const HRESULT thread = CheckThread();
    if (FAILED(thread)) return thread;
    const HRESULT verified = VerifyPrimaryHorizontalTaskbar(taskbarWindow);
    if (FAILED(verified)) {
        RecordFailure(operation, verified);
        return verified;
    }
    return AttachTreeFromAbi(rootContentAbi, TRUE, operation);
}

HRESULT AttachFromAbi(void* trayAbi, BOOL primaryHorizontalTaskbar, wchar_t const* operation) noexcept {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) {
        OutputDebugStringW(L"BatteryHub taskbar XAML refused a call from the wrong thread.\n");
        return hr;
    }
    if (g_disabled) return HRESULT_FROM_WIN32(ERROR_SERVICE_DISABLED);
    if (!primaryHorizontalTaskbar) hr = HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    else if (!trayAbi) hr = E_POINTER;
    else {
        try {
            IInspectable tray{ nullptr };
            copy_from_abi(tray, trayAbi);
            if (g_trayObject && g_trayObject == tray && g_widget && (g_individual || g_slot.IsAttached())) {
                g_failures = 0;
                Log(L"already-attached");
                return S_FALSE;
            }
            if (g_trayObject && !RemoveAllSlots()) {
                hr = E_FAIL;
            } else {
                g_trayObject = nullptr;
                g_widget = nullptr;
                g_surfaceView.reset();
                hr = AttachCore(tray);
            }
        } catch (hresult_error const& e) {
            hr = e.code();
        } catch (...) {
            hr = E_FAIL;
        }
    }
    if (FAILED(hr)) RecordFailure(operation, hr);
    else { g_failures = 0; Log(operation, hr); }
    return hr;
}
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(instance);
    }
    return TRUE;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlAttach(void* trayAbi, BOOL primaryHorizontalTaskbar) {
    return AttachFromAbi(trayAbi, primaryHorizontalTaskbar, L"attach");
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlAttachTree(void* rootContentAbi, BOOL primaryHorizontalTaskbar) {
    return AttachTreeFromAbi(rootContentAbi, primaryHorizontalTaskbar, L"attach-tree");
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlAttachVerifiedTaskbarTree(void* rootContentAbi, HWND taskbarWindow) {
    return AttachVerifiedTaskbarTree(rootContentAbi, taskbarWindow, L"attach-verified-taskbar-tree");
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlTreeRebuilt(void* newTrayAbi, BOOL primaryHorizontalTaskbar) {
    return AttachFromAbi(newTrayAbi, primaryHorizontalTaskbar, L"tree-rebuilt");
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlTreeRebuiltTree(void* rootContentAbi, BOOL primaryHorizontalTaskbar) {
    return AttachTreeFromAbi(rootContentAbi, primaryHorizontalTaskbar, L"tree-rebuilt-tree");
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlTreeRebuiltVerifiedTaskbarTree(void* rootContentAbi, HWND taskbarWindow) {
    return AttachVerifiedTaskbarTree(rootContentAbi, taskbarWindow, L"tree-rebuilt-verified-taskbar-tree");
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlDetach() {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) { RecordFailure(L"detach-wrong-thread", hr); return hr; }
    if (!g_trayObject) { Log(L"detach-noop"); return S_FALSE; }
    if (!RemoveAllSlots()) {
        RecordFailure(L"detach-failed", E_FAIL);
        return E_FAIL;
    }
    g_trayObject = nullptr;
    g_widget = nullptr;
    g_surfaceView.reset();
    Log(L"detach");
    return S_OK;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlResetCircuitBreaker() {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    if (g_trayObject) return HRESULT_FROM_WIN32(ERROR_BUSY);
    g_failures = 0;
    g_disabled = false;
    Log(L"circuit-reset");
    return S_OK;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlGetStatus(BatteryHubTaskbarXamlStatus* status) {
    if (!status) return E_POINTER;
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    status->consecutiveFailures = g_failures;
    status->attached = g_trayObject ? 1 : 0;
    status->disabled = g_disabled ? 1 : 0;
    return S_OK;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlSetLogPath(wchar_t const* absolutePath) {
    if (g_taskbarThread != 0 && g_taskbarThread != GetCurrentThreadId()) return RPC_E_WRONG_THREAD;
    if (!absolutePath || !*absolutePath || wcslen(absolutePath) >= 32760 ||
        PathIsRelativeW(absolutePath)) return E_INVALIDARG;
    g_logPath = absolutePath;
    Log(L"log-path-set");
    return S_OK;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlSetInputTarget(HWND window, UINT message) {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    if (!g_surfaceView || !window || !IsWindow(window) || !message) return E_INVALIDARG;
    g_surfaceView->SetInputTarget(window, message);
    return S_OK;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlUpdateSurface(
    std::uint32_t const* premultipliedBgra, std::uint32_t widthPx,
    std::uint32_t heightPx, std::uint32_t dpi,
    widget_bridge::Rect const* itemRects, std::uint64_t const* itemIds,
    std::int32_t const* itemAnchors, std::uint32_t itemCount, BOOL individual,
    std::uint64_t sessionId, std::uint32_t generation) {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    if (!g_surfaceView || !g_trayObject || (itemCount && (!itemRects || !itemIds || !itemAnchors))) {
        hr = HRESULT_FROM_WIN32(ERROR_NOT_READY);
        Log(L"frame-update-not-ready-no-attached-xaml-slot", hr);
        return hr;
    }
    hr = g_surfaceView->Update(premultipliedBgra, widthPx, heightPx, dpi, itemRects,
                               itemIds, itemAnchors, individual != FALSE, itemCount,
                               sessionId, generation);
    if (FAILED(hr)) RecordFailure(L"surface-update", hr);
    else g_failures = 0;
    LogSurface(L"writeablebitmap-update", hr);
    return hr;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlSetWidgetLayout(
    BOOL individual, std::uint64_t const* itemIds, std::int32_t const* itemAnchors,
    std::uint32_t itemCount, std::int32_t desiredGridColumn, BOOL positionLocked) {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    if (desiredGridColumn < widget_bridge::kMinGridAnchor ||
        desiredGridColumn >= static_cast<std::int32_t>(widget_bridge::kMaxGridColumns) ||
        !g_trayObject || !g_surfaceView || itemCount > widget_bridge::kMaxItems ||
        (itemCount && (!itemIds || !itemAnchors)))
        return E_INVALIDARG;
    if (individual != FALSE) {
        for (std::uint32_t i = 0; i < itemCount; ++i) {
            if (itemAnchors[i] < widget_bridge::kMinGridAnchor ||
                itemAnchors[i] >= static_cast<std::int32_t>(widget_bridge::kMaxGridColumns)) return E_INVALIDARG;
            if (itemIds[i]) for (std::uint32_t j = 0; j < i; ++j)
                if (itemIds[j] == itemIds[i]) return HRESULT_FROM_WIN32(ERROR_DUP_NAME);
        }
        bool rebuild = !g_individual || itemCount != g_layoutIds.size();
        for (std::uint32_t i = 0; !rebuild && i < itemCount; ++i)
            rebuild = g_layoutIds[i] != itemIds[i] || g_layoutAnchors[i] != itemAnchors[i];
        if (rebuild) {
            if (g_slot.IsAttached() && !g_slot.Remove()) return E_FAIL;
            if (!RemoveModuleSlots()) return E_FAIL;
            g_individual = true;
            if (!BuildModuleSlots(itemIds, itemAnchors, itemCount)) {
                g_individual = false;
                InsertGroupedSlot();
                return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
            }
        }
        const bool canDrag = positionLocked == FALSE && std::any_of(g_moduleSlots.begin(), g_moduleSlots.end(),
            [](auto const& item) { return item.slot && item.slot->AnchorCount() > 1; });
        g_surfaceView->SetDragEnabled(canDrag);
        Log(canDrag ? L"individual-modules-attached-draggable" : L"individual-modules-attached-locked", S_OK);
        return S_OK;
    }

    if (g_individual) {
        if (!RemoveModuleSlots()) return E_FAIL;
        g_individual = false;
    }
    if (!g_slot.IsAttached() && !InsertGroupedSlot()) return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    bool moved = g_slot.SetDesiredColumn(desiredGridColumn);
    if (!moved && desiredGridColumn != -1) {
        moved = g_slot.SetDesiredColumn(-1); // Keep the preference; use the chevron until its anchor returns.
        g_surfaceView->SetDragEnabled(positionLocked == FALSE && moved && g_slot.AnchorCount() > 1);
        return S_FALSE;
    }
    const bool dragEnabled = positionLocked == FALSE && moved && g_slot.AnchorCount() > 1;
    g_surfaceView->SetDragEnabled(dragEnabled);
    wchar_t trace[192]{};
    swprintf_s(trace, L"drag-config locked=%u enabled=%u desired=%d anchors=%llu moved=%u",
               positionLocked != FALSE ? 1u : 0u,
               dragEnabled ? 1u : 0u,
               desiredGridColumn,
               static_cast<unsigned long long>(g_slot.AnchorCount()), moved ? 1u : 0u);
    Log(trace, moved ? S_OK : S_FALSE);
    // The original chevron insertion remains valid even if its shared Grid column cannot be
    // represented as a distinct safe anchor. Keep presentation working and disable only dragging.
    return moved ? S_OK : S_FALSE;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlSetWidgetPosition(std::int32_t desiredGridColumn, BOOL positionLocked) {
    return BatteryHubTaskbarXamlSetWidgetLayout(FALSE, nullptr, nullptr, 0, desiredGridColumn, positionLocked);
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlGetWidgetScreenRect(HWND taskbarWindow, RECT* screenRect) {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    if (!screenRect || !g_surfaceView || (!g_individual && !g_slot.IsAttached()) ||
        (g_individual && g_moduleSlots.empty())) return E_INVALIDARG;
    hr = g_surfaceView->GetScreenRect(taskbarWindow, *screenRect);
    LogSurface(L"xaml-geometry-ready", hr, SUCCEEDED(hr) ? screenRect : nullptr);
    return hr;
}

extern "C" HRESULT WINAPI BatteryHubTaskbarXamlSetWidgetVisible(BOOL visible) {
    HRESULT hr = CheckThread();
    if (FAILED(hr)) return hr;
    if (!g_surfaceView || (!g_individual && !g_slot.IsAttached()) ||
        (g_individual && g_moduleSlots.empty())) {
        hr = HRESULT_FROM_WIN32(ERROR_NOT_READY);
        Log(visible ? L"show-not-ready-no-attached-xaml-slot" : L"hide-not-ready-no-attached-xaml-slot", hr);
        return hr;
    }
    hr = g_surfaceView->SetSurfaceVisible(visible != FALSE);
    LogSurface(visible ? L"xaml-visible-requested" : L"xaml-hidden", hr);
    return hr;
}
