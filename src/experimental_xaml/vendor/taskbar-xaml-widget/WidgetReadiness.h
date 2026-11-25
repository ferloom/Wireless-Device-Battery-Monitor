#pragma once

namespace batteryhub::xaml {

enum class WidgetReadiness {
    Disconnected,
    WaitingForFrame,
    FrameValid,
    SurfaceReady,
    HostReady,
    Failed
};

struct ReadinessInputs {
    bool connected = false;
    bool frameValid = false;
    bool surfaceReady = false;
    bool presentationReady = false;
    bool hostVisible = false;
    bool hostGeometryValid = false;
    bool anchorValid = false;
    bool horizontalTaskbar = false;
    bool failed = false;
};

// A global render event is a one-shot presentation witness, not the definition
// of a usable XAML surface. Layout and host visibility are checked separately.
struct PresentationFence {
    bool visible = false;
    bool pending = false;
    bool presented = false;

    constexpr bool SetVisible(bool next) noexcept {
        if (visible == next) return false;
        visible = next;
        pending = next;
        presented = false;
        return true;
    }

    constexpr void RequireFreshRender() noexcept {
        pending = visible;
        presented = false;
    }

    constexpr bool OnRenderPass(bool layoutReady, bool hostVisible) noexcept {
        if (!visible || !pending || !layoutReady || !hostVisible) return false;
        pending = false;
        presented = true;
        return true;
    }

    constexpr bool NeedsObservation() const noexcept {
        return visible && pending;
    }
};

constexpr bool HostCanPresent(bool xamlHostVisible, bool taskbarWindowVisible,
                              bool surfaceUncovered, bool previouslyCommitted) noexcept {
    return xamlHostVisible && taskbarWindowVisible && (surfaceUncovered || previouslyCommitted);
}

constexpr WidgetReadiness EvaluateReadiness(ReadinessInputs const& input) noexcept {
    if (!input.connected) return WidgetReadiness::Disconnected;
    if (input.failed) return WidgetReadiness::Failed;
    if (!input.frameValid) return WidgetReadiness::WaitingForFrame;
    if (!input.surfaceReady) return WidgetReadiness::FrameValid;
    if (!input.presentationReady || !input.hostVisible || !input.hostGeometryValid ||
        !input.anchorValid || !input.horizontalTaskbar)
        return WidgetReadiness::SurfaceReady;
    return WidgetReadiness::HostReady;
}

constexpr bool TraditionalWidgetVisible(WidgetReadiness state) noexcept {
    return state != WidgetReadiness::HostReady;
}

constexpr bool HostXamlWidgetVisible(WidgetReadiness state) noexcept {
    return state == WidgetReadiness::HostReady;
}

constexpr bool CommittedVisibilityInvariant(WidgetReadiness state) noexcept {
    return TraditionalWidgetVisible(state) != HostXamlWidgetVisible(state);
}

} // namespace batteryhub::xaml
