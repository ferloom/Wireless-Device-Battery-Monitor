#define WIN32_LEAN_AND_MEAN
#include <windows.h>

#include <atomic>
#include <cstdio>

namespace {
constexpr wchar_t kClassName[] = L"BatteryHubHookDeliveryTestWindow";
constexpr UINT kSentMessage = WM_APP + 0x701;
constexpr UINT kPostedMessage = WM_APP + 0x702;
std::atomic<LONG> g_callSent{0}, g_callPosted{0}, g_getSent{0}, g_getPosted{0};
HWND g_window = nullptr;
HANDLE g_delivered = nullptr;

LRESULT CALLBACK WindowProc(HWND hwnd, UINT message, WPARAM wParam, LPARAM lParam) {
    if (message == kPostedMessage) { SetEvent(g_delivered); return 0; }
    if (message == WM_CLOSE) { DestroyWindow(hwnd); return 0; }
    if (message == WM_DESTROY) { PostQuitMessage(0); return 0; }
    return DefWindowProcW(hwnd, message, wParam, lParam);
}

DWORD WINAPI WindowThread(void* parameter) {
    auto ready = static_cast<HANDLE>(parameter);
    WNDCLASSW wc{};
    wc.lpfnWndProc = WindowProc;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.lpszClassName = kClassName;
    if (!RegisterClassW(&wc) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
        SetEvent(ready);
        return 1;
    }
    g_window = CreateWindowExW(0, kClassName, L"", WS_POPUP, 0, 0, 1, 1,
                               nullptr, nullptr, wc.hInstance, nullptr);
    SetEvent(ready);
    if (!g_window) return 2;
    MSG message{};
    while (GetMessageW(&message, nullptr, 0, 0) > 0) DispatchMessageW(&message);
    DestroyWindow(g_window);
    g_window = nullptr;
    return 0;
}

LRESULT CALLBACK CallWndHook(int code, WPARAM wParam, LPARAM lParam) {
    if (code == HC_ACTION && lParam) {
        auto const* message = reinterpret_cast<CWPSTRUCT const*>(lParam);
        if (message->hwnd == g_window && message->message == kSentMessage) ++g_callSent;
        if (message->hwnd == g_window && message->message == kPostedMessage) ++g_callPosted;
    }
    return CallNextHookEx(nullptr, code, wParam, lParam);
}

LRESULT CALLBACK GetMessageHook(int code, WPARAM wParam, LPARAM lParam) {
    if (code == HC_ACTION && wParam == PM_REMOVE && lParam) {
        auto const* message = reinterpret_cast<MSG const*>(lParam);
        if (message->hwnd == g_window && message->message == kSentMessage) ++g_getSent;
        if (message->hwnd == g_window && message->message == kPostedMessage) ++g_getPosted;
    }
    return CallNextHookEx(nullptr, code, wParam, lParam);
}
}

int main() {
    HANDLE ready = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    g_delivered = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (!ready || !g_delivered) return 1;
    DWORD threadId = 0;
    HANDLE thread = CreateThread(nullptr, 0, WindowThread, ready, 0, &threadId);
    if (!thread || WaitForSingleObject(ready, 5000) != WAIT_OBJECT_0 || !g_window) return 2;

    HHOOK callHook = SetWindowsHookExW(WH_CALLWNDPROC, CallWndHook, nullptr, threadId);
    HHOOK getHook = SetWindowsHookExW(WH_GETMESSAGE, GetMessageHook, nullptr, threadId);
    if (!callHook || !getHook) return 3;

    SendMessageW(g_window, kSentMessage, 0, 0);
    if (!PostMessageW(g_window, kPostedMessage, 0, 0)) return 4;
    if (WaitForSingleObject(g_delivered, 5000) != WAIT_OBJECT_0) return 4;

    const LONG callSent = g_callSent.load(), callPosted = g_callPosted.load();
    const LONG getSent = g_getSent.load(), getPosted = g_getPosted.load();
    UnhookWindowsHookEx(getHook);
    UnhookWindowsHookEx(callHook);
    PostMessageW(g_window, WM_CLOSE, 0, 0);
    WaitForSingleObject(thread, 5000);
    CloseHandle(thread);
    CloseHandle(ready);
    CloseHandle(g_delivered);

    if (callSent != 1 || callPosted != 0 || getSent != 0 || getPosted != 1) {
        std::fprintf(stderr, "FAIL: CALLWNDPROC sent=%ld posted=%ld; GETMESSAGE sent=%ld posted=%ld\n",
                     callSent, callPosted, getSent, getPosted);
        return 5;
    }
    std::puts("PASS: WH_CALLWNDPROC sees SendMessage; WH_GETMESSAGE sees PostMessage");
    return 0;
}
