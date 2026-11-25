#pragma once

#include <cstddef>
#include <cstdint>

namespace widget_bridge {

constexpr std::uint32_t kMagic = 0x42574858; // BWHX
constexpr std::uint32_t kVersion = 5;
constexpr std::uint32_t kMaxWidth = 8192;
constexpr std::uint32_t kMaxHeight = 256;
constexpr std::uint32_t kMaxItems = 64;
constexpr std::uint32_t kMaxGridColumns = 4096;
constexpr std::int32_t kMinGridAnchor = -static_cast<std::int32_t>(kMaxGridColumns) - 1;
constexpr std::uint32_t kMaxPixels = kMaxWidth * kMaxHeight;
constexpr wchar_t kMappingPrefix[] = L"Local\\BatteryHub.Experimental.WidgetSurface.v4.";
constexpr wchar_t kSurfaceChangedMessage[] = L"BatteryHub.Experimental.WidgetSurface.Changed.v4";

enum class IntegrationState : std::int32_t { Traditional = 0, Requested = 1, Ready = 2, Failed = 3 };
constexpr bool CanReuseInactiveMapping(bool ownerExited, bool requested, IntegrationState state) noexcept {
    return ownerExited && !requested &&
           (state == IntegrationState::Traditional || state == IntegrationState::Failed);
}
enum class InputKind : std::uint16_t {
    None = 0, Enter = 1, Move = 2, Leave = 3, LeftClick = 4, RightClick = 5, DragCommit = 6,
    DeviceDragStart = 7, DeviceDragMove = 8, DeviceDragStop = 9, DeviceDragCancel = 10,
    ModuleDragCommit = 11, ModuleHover = 12, ModuleLeave = 13
};
constexpr std::uint16_t kNoInputItem = 0xffffu;

constexpr bool ValidInput(InputKind kind, int item) noexcept {
    return kind >= InputKind::Enter && kind <= InputKind::ModuleLeave &&
           (kind == InputKind::DragCommit
                ? item >= kMinGridAnchor && item < static_cast<int>(kMaxGridColumns)
                : item >= -1 && item < static_cast<int>(kMaxItems));
}

constexpr std::uint32_t EncodeInput(InputKind kind, int item) noexcept {
    if (!ValidInput(kind, item)) return 0u;
    const auto encodedItem = kind == InputKind::DragCommit && item < -1
                                 ? static_cast<std::uint16_t>(0x8000u | static_cast<unsigned>(-item))
                                 : static_cast<std::uint16_t>(item < 0 ? kNoInputItem : item);
    return static_cast<std::uint32_t>(kind) | (static_cast<std::uint32_t>(encodedItem) << 16);
}

constexpr bool DecodeInput(std::uint32_t packed, InputKind& kind, int& item) noexcept {
    const auto rawKind = static_cast<std::uint16_t>(packed);
    const auto rawItem = static_cast<std::uint16_t>(packed >> 16);
    const auto decodedKind = static_cast<InputKind>(rawKind);
    const int decodedItem = rawItem == kNoInputItem ? -1
                          : decodedKind == InputKind::DragCommit && (rawItem & 0x8000u)
                              ? -static_cast<int>(rawItem & 0x7fffu)
                              : rawItem;
    if (!ValidInput(decodedKind, decodedItem)) return false;
    kind = decodedKind;
    item = decodedItem;
    return true;
}

constexpr bool DragAnchorRoundTrips(int item) noexcept {
    const auto packed = EncodeInput(InputKind::DragCommit, item);
    InputKind kind{};
    int decoded = 0;
    return packed && DecodeInput(packed, kind, decoded) && kind == InputKind::DragCommit && decoded == item;
}
static_assert(DragAnchorRoundTrips(-4097) && DragAnchorRoundTrips(-2) && DragAnchorRoundTrips(-1) &&
              DragAnchorRoundTrips(0) && DragAnchorRoundTrips(4095));
static_assert(EncodeInput(InputKind::DragCommit, -4098) == 0 && EncodeInput(InputKind::Move, -2) == 0);

struct Rect {
    std::int32_t left, top, right, bottom;
};

// Shared memory contains only the finished widget surface and hit-test geometry. The XAML module never
// opens HID devices. App-written fields are guarded by snapshotSequence; Explorer writes the separate
// state, slot and input fields with Interlocked operations.
struct SharedSurface {
    std::uint32_t magic;
    std::uint32_t version;
    std::uint64_t sessionId; // new for every publisher instance; generation identifies a frame within it
    volatile std::int32_t snapshotSequence;
    std::uint32_t ownerPid;
    std::uint32_t targetPid;
    std::uint32_t ownerSession;
    std::uint32_t targetSession;
    std::uint64_t ownerWindow;
    std::uint64_t taskbarWindow;
    volatile std::int32_t requested;
    volatile std::int32_t integrationState;
    std::uint32_t generation;
    std::uint32_t dpi;
    std::uint32_t widthPx;
    std::uint32_t heightPx;
    std::uint32_t itemCount;
    std::uint32_t pixelBytes;
    std::uint32_t flags; // bit 0: light; bit 1: hover; bit 2: locked; bit 3: Individual XAML modules
    std::int32_t desiredGridColumn; // -1 is the default; <=-2 encodes before a Grid column
    Rect itemRects[kMaxItems];
    std::uint64_t itemIds[kMaxItems]; // stable FNV-1a IDs derived from device keys; zero is a non-device item
    std::int32_t itemAnchors[kMaxItems]; // logical before/after anchor; -1 selects the default slot
    volatile std::int32_t slotSequence;
    Rect slotScreenRect;
    volatile std::int32_t inputSequence;
    volatile std::int64_t integrationHeartbeatMs;
    std::uint32_t inputKind;
    std::uint32_t inputItem;
    std::int32_t inputX;
    std::int32_t inputY;
    std::uint32_t reserved;
    std::uint32_t pixels[kMaxPixels]; // premultiplied BGRA from BatteryHub's existing PaintStrip()
};

constexpr std::size_t kMappingBytes = sizeof(SharedSurface);

inline bool ValidSurface(std::uint32_t width, std::uint32_t height, std::uint32_t bytes,
                         std::uint32_t itemCount, std::uint32_t dpi) noexcept {
    if (!width || width > kMaxWidth || !height || height > kMaxHeight || itemCount > kMaxItems ||
        dpi < 48 || dpi > 768) return false;
    const std::uint64_t expected = static_cast<std::uint64_t>(width) * height * sizeof(std::uint32_t);
    return expected <= static_cast<std::uint64_t>(kMaxPixels) * sizeof(std::uint32_t) && bytes == expected;
}

inline bool ValidRect(Rect const& rect, std::uint32_t width, std::uint32_t height) noexcept {
    return rect.left >= 0 && rect.top >= 0 && rect.right >= rect.left && rect.bottom >= rect.top &&
           static_cast<std::uint32_t>(rect.right) <= width && static_cast<std::uint32_t>(rect.bottom) <= height;
}

inline bool ValidFrame(SharedSurface const& surface) noexcept {
    if (surface.magic != kMagic || surface.version != kVersion ||
        !surface.sessionId ||
        (surface.flags & ~15u) != 0 || surface.desiredGridColumn < kMinGridAnchor ||
        surface.desiredGridColumn >= static_cast<int>(kMaxGridColumns) ||
        !ValidSurface(surface.widthPx, surface.heightPx, surface.pixelBytes,
                      surface.itemCount, surface.dpi)) return false;
    for (std::uint32_t i = 0; i < surface.itemCount; ++i)
        if (!ValidRect(surface.itemRects[i], surface.widthPx, surface.heightPx) ||
            surface.itemAnchors[i] < kMinGridAnchor || surface.itemAnchors[i] >= static_cast<int>(kMaxGridColumns)) return false;
        else if (surface.itemIds[i])
            for (std::uint32_t j = 0; j < i; ++j)
                if (surface.itemIds[j] == surface.itemIds[i]) return false;
    return true;
}

inline bool ValidSlot(Rect const& rect, std::uint32_t widthPx, std::uint32_t heightPx) noexcept {
    const auto width = static_cast<std::int64_t>(rect.right) - rect.left;
    const auto height = static_cast<std::int64_t>(rect.bottom) - rect.top;
    return width > 0 && height > 0 && static_cast<std::uint64_t>(width) == widthPx &&
           static_cast<std::uint64_t>(height) == heightPx;
}

inline bool ValidSlot(Rect const& rect, SharedSurface const& surface) noexcept {
    if (!(surface.flags & 8u)) return ValidSlot(rect, surface.widthPx, surface.heightPx);
    const auto width = static_cast<std::int64_t>(rect.right) - rect.left;
    const auto height = static_cast<std::int64_t>(rect.bottom) - rect.top;
    std::uint64_t moduleWidth = 0;
    for (std::uint32_t i = 0; i < surface.itemCount; ++i) {
        const auto itemWidth = static_cast<std::int64_t>(surface.itemRects[i].right) - surface.itemRects[i].left;
        if (itemWidth <= 0 || moduleWidth > UINT64_MAX - static_cast<std::uint64_t>(itemWidth)) return false;
        moduleWidth += static_cast<std::uint64_t>(itemWidth);
    }
    return width > 0 && static_cast<std::uint64_t>(width) >= moduleWidth &&
           height == static_cast<std::int64_t>(surface.heightPx);
}

inline bool ShouldHideTraditional(IntegrationState state, bool requested, bool validContent,
                                 bool validSlot, bool freshHeartbeat) noexcept {
    return requested && validContent && validSlot && freshHeartbeat && state == IntegrationState::Ready;
}

constexpr bool CanKeepPreviousReadySlot(bool previouslyReady, IntegrationState state,
                                        bool requested, bool freshHeartbeat) noexcept {
    return previouslyReady && requested && freshHeartbeat && state == IntegrationState::Ready;
}

inline std::uint64_t PixelHash(std::uint32_t const* pixels, std::size_t count) noexcept {
    if ((!pixels && count) || count > SIZE_MAX / sizeof(std::uint32_t)) return 0;
    std::uint64_t hash = 14695981039346656037ull;
    auto const* bytes = reinterpret_cast<unsigned char const*>(pixels);
    for (std::size_t i = 0; i < count * sizeof(std::uint32_t); ++i) {
        hash ^= bytes[i];
        hash *= 1099511628211ull;
    }
    return hash;
}

inline bool ValidPremultipliedBgra(std::uint32_t const* pixels, std::size_t count) noexcept {
    if (!pixels) return false;
    for (std::size_t i = 0; i < count; ++i) {
        const auto p = pixels[i];
        const auto a = p >> 24;
        if (((p >> 16) & 255u) > a || ((p >> 8) & 255u) > a || (p & 255u) > a) return false;
    }
    return true;
}

} // namespace widget_bridge
