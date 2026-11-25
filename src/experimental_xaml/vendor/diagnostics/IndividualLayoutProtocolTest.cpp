#include "../publisher/widget_bridge_protocol.h"

#include <cstdio>
#include <memory>

using namespace widget_bridge;

int main() {
    if (!CanReuseInactiveMapping(true, false, IntegrationState::Traditional) ||
        !CanReuseInactiveMapping(true, false, IntegrationState::Failed) ||
        CanReuseInactiveMapping(false, false, IntegrationState::Failed) ||
        CanReuseInactiveMapping(true, true, IntegrationState::Failed) ||
        CanReuseInactiveMapping(true, false, IntegrationState::Ready)) return 7;
    if (!CanKeepPreviousReadySlot(true, IntegrationState::Ready, true, true) ||
        CanKeepPreviousReadySlot(false, IntegrationState::Ready, true, true) ||
        CanKeepPreviousReadySlot(true, IntegrationState::Requested, true, true) ||
        CanKeepPreviousReadySlot(true, IntegrationState::Ready, false, true) ||
        CanKeepPreviousReadySlot(true, IntegrationState::Ready, true, false)) return 8;

    for (auto kind : {InputKind::ModuleDragCommit, InputKind::ModuleHover, InputKind::ModuleLeave}) {
        const auto packed = EncodeInput(kind, 1);
        InputKind decoded{};
        int item = -1;
        if (!packed || !DecodeInput(packed, decoded, item) || decoded != kind || item != 1) return 1;
    }

    auto frame = std::make_unique<SharedSurface>();
    frame->magic = kMagic;
    frame->version = kVersion;
    frame->sessionId = 1;
    frame->flags = 8u;
    frame->desiredGridColumn = -1;
    frame->widthPx = 20;
    frame->heightPx = 10;
    frame->dpi = 96;
    frame->itemCount = 2;
    frame->pixelBytes = 20u * 10u * sizeof(std::uint32_t);
    frame->itemRects[0] = {0, 0, 10, 10};
    frame->itemRects[1] = {10, 0, 20, 10};
    frame->itemIds[0] = 0x1001;
    frame->itemIds[1] = 0x1002;
    frame->itemAnchors[0] = -2;
    frame->itemAnchors[1] = 2;
    if (!ValidFrame(*frame)) return 2;
    if (!ValidSlot({100, 100, 120, 110}, *frame) || ValidSlot({100, 100, 119, 110}, *frame)) return 3;

    frame->itemIds[1] = frame->itemIds[0];
    if (ValidFrame(*frame)) return 4;
    frame->itemIds[1] = 0x1002;
    frame->itemAnchors[1] = kMinGridAnchor - 1;
    if (ValidFrame(*frame)) return 5;
    frame->itemAnchors[1] = 2;
    frame->flags = 0;
    if (!ValidSlot({100, 100, 120, 110}, *frame) || ValidSlot({100, 100, 150, 110}, *frame)) return 6;

    std::puts("PASS: inactive mapping recovery, keyed Individual frames, module input, committed-slot transitions, and validation");
    return 0;
}
