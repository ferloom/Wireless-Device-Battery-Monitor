#pragma once

#include <winrt/Windows.Foundation.Collections.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include "../publisher/widget_bridge_protocol.h"

#include <algorithm>
#include <utility>
#include <vector>

// Inserts only BatteryHub-owned elements. Native children keep their order and original columns.
class TaskbarSlot {
public:
    bool IsAttached() const noexcept { return m_kind != Kind::None; }

    bool InsertAfter(winrt::Windows::UI::Xaml::Controls::Grid const& tray,
                     wchar_t const* anchorName,
                     winrt::Windows::UI::Xaml::FrameworkElement const& item) {
        using namespace winrt::Windows::UI::Xaml;
        using namespace winrt::Windows::UI::Xaml::Controls;
        using namespace winrt::Windows::UI::Xaml::Media;
        if (m_kind != Kind::None || !tray || !item) return false;
        FrameworkElement anchor{ nullptr };
        for (auto const& child : tray.Children()) {
            auto element = child.try_as<FrameworkElement>();
            if (element && element.Name() == anchorName) { anchor = element; break; }
        }
        if (!anchor) return false;
        m_defaultColumn = Grid::GetColumn(anchor);
        m_grid = tray;
        m_item = item;
        try {
            m_originalColumns.reserve(tray.Children().Size());
            for (auto const& child : tray.Children()) {
                auto element = child.try_as<FrameworkElement>();
                if (!element) continue;
                const int column = Grid::GetColumn(element);
                m_originalColumns.emplace_back(element, column);
            }
            for (auto const& saved : m_originalColumns) {
                if (saved.first == item || IsOwned(saved.first) || Grid::GetRow(saved.first) != Grid::GetRow(anchor) ||
                    Grid::GetRowSpan(saved.first) != 1 || Grid::GetColumnSpan(saved.first) != 1) continue;
                const int column = saved.second;
                if (column < 0 || column >= static_cast<int>(widget_bridge::kMaxGridColumns) ||
                    column >= static_cast<int>(tray.ColumnDefinitions().Size())) continue;
                const auto count = std::count_if(m_originalColumns.begin(), m_originalColumns.end(),
                    [column, &item](auto const& value) { return value.first != item && value.second == column; });
                if (count == 1) m_anchors.push_back({column, saved.first});
            }
            if (m_anchors.empty()) { Reset(); return false; }
            std::sort(m_anchors.begin(), m_anchors.end(), [](auto const& a, auto const& b) { return a.first < b.first; });
            m_anchorRow = Grid::GetRow(anchor);
            m_kind = Kind::Grid;
            if (!MoveToColumn(Grid::GetColumn(anchor))) { Remove(); return false; }
            CreateMarker();
            Grid::SetRow(m_marker, m_anchorRow);
            m_grid.Children().Append(m_marker);
            return true;
        } catch (...) { Remove(); return false; }
    }

    bool InsertAfter(winrt::Windows::UI::Xaml::Controls::StackPanel const& tray,
                     wchar_t const* anchorName,
                     winrt::Windows::UI::Xaml::FrameworkElement const& item) {
        using namespace winrt::Windows::UI::Xaml;
        if (m_kind != Kind::None || !tray || !item ||
            tray.Orientation() != Controls::Orientation::Horizontal) return false;
        uint32_t index = 0;
        bool found = false;
        FrameworkElement anchor{ nullptr };
        for (auto const& child : tray.Children()) {
            auto element = child.try_as<FrameworkElement>();
            if (element && element.Name() == anchorName) {
                found = tray.Children().IndexOf(child, index);
                anchor = element;
                break;
            }
        }
        if (!found) return false;
        try {
            m_stack = tray;
            m_item = item;
            m_kind = Kind::StackPanel;
            // Keep every direct sibling as a logical anchor. The sentinel is the position before
            // the first native group; never descend into an Explorer-owned item collection.
            m_stackAnchors.push_back(nullptr);
            for (uint32_t i = 0; i < tray.Children().Size(); ++i) {
                auto element = tray.Children().GetAt(i).try_as<FrameworkElement>();
                if (element && element != item && !IsOwned(element)) m_stackAnchors.push_back(element);
            }
            if (m_stackAnchors.empty()) { Reset(); return false; }
            m_stackRightToLeft = tray.FlowDirection() == FlowDirection::RightToLeft;
            // m_stackAnchors[0] is the before-first sentinel; child index N maps to N+1.
            // The default slot immediately after NotifyIconStack (the chevron) is therefore N+1.
            auto anchorIt = std::find(m_stackAnchors.begin(), m_stackAnchors.end(), anchor);
            if (anchorIt == m_stackAnchors.end()) { Remove(); return false; }
            m_defaultColumn = static_cast<int>(anchorIt - m_stackAnchors.begin());
            CreateMarker();
            if (!MoveToStackAnchor(m_defaultColumn)) { Remove(); return false; }
            return true;
        } catch (...) { Remove(); return false; }
    }

    bool HasAnchors() const noexcept { return (m_grid && !m_anchors.empty()) || (m_stack && !m_stackAnchors.empty()); }
    std::size_t AnchorCount() const noexcept { return m_grid ? m_anchors.size() : m_stackAnchors.size(); }
    winrt::Windows::UI::Xaml::FrameworkElement TrayElement() const {
        if (m_grid) return m_grid.as<winrt::Windows::UI::Xaml::FrameworkElement>();
        if (m_stack) return m_stack.as<winrt::Windows::UI::Xaml::FrameworkElement>();
        return nullptr;
    }

    int PreviewAt(double xInTray) noexcept {
        return PreviewAt(xInTray, false);
    }

    int PreviewAnchorAt(double xInTray) noexcept {
        return PreviewAt(xInTray, true);
    }

    int PreviewAt(double xInTray, bool logical) noexcept {
        using namespace winrt::Windows::UI::Xaml;
        using namespace winrt::Windows::UI::Xaml::Controls;
        if (m_kind == Kind::StackPanel && m_stack && !m_stackAnchors.empty() && m_marker) {
            try {
                if (m_anchorEdges.size() != m_stackAnchors.size()) {
                    m_anchorEdges.clear();
                    m_anchorEdges.reserve(m_stackAnchors.size());
                    for (auto const& anchor : m_stackAnchors) {
                        if (anchor) {
                            const double left = anchor.TransformToVisual(m_stack).TransformPoint({0, 0}).X;
                            m_anchorEdges.push_back(m_stackRightToLeft ? left : left + anchor.ActualWidth());
                        } else {
                            auto first = m_stackAnchors.size() > 1 ? m_stackAnchors[1] : nullptr;
                            if (first) {
                                const double left = first.TransformToVisual(m_stack).TransformPoint({0, 0}).X;
                                m_anchorEdges.push_back(m_stackRightToLeft ? left + first.ActualWidth() : left);
                            } else {
                                m_anchorEdges.push_back(0.0);
                            }
                        }
                    }
                }
                std::size_t best = 0;
                double bestDistance = 1.0e30;
                for (std::size_t i = 0; i < m_anchorEdges.size(); ++i) {
                    const double distance = std::abs(m_anchorEdges[i] - xInTray);
                    if (distance < bestDistance) { bestDistance = distance; best = i; }
                }
                if (m_previewAnchor == static_cast<int>(best)) return m_previewAnchor;
                uint32_t markerIndex = 0;
                if (m_stack.Children().IndexOf(m_marker, markerIndex)) m_stack.Children().RemoveAt(markerIndex);
                uint32_t anchorIndex = 0;
                if (m_stackAnchors[best]) {
                    if (!m_stack.Children().IndexOf(m_stackAnchors[best], anchorIndex)) return -1;
                    m_stack.Children().InsertAt(anchorIndex + 1, m_marker);
                } else {
                    m_stack.Children().InsertAt(0, m_marker);
                }
                m_marker.Visibility(Visibility::Visible);
                m_previewColumn = static_cast<int>(best);
                m_previewAnchor = static_cast<int>(best);
                return m_previewColumn;
            } catch (...) { return -1; }
        }
        if (m_kind != Kind::Grid || !m_grid || m_anchors.empty() || !m_marker) return -1;
        try {
            if (m_anchorEdges.size() != m_anchors.size() * 2) {
                m_anchorEdges.clear();
                m_anchorEdges.reserve(m_anchors.size() * 2);
                const bool rtl = m_grid.FlowDirection() == FlowDirection::RightToLeft;
                for (auto const& anchor : m_anchors) {
                    const double left = anchor.second.TransformToVisual(m_grid).TransformPoint({0, 0}).X;
                    const double right = left + anchor.second.ActualWidth();
                    m_anchorEdges.push_back(rtl ? right : left); // before this column
                    m_anchorEdges.push_back(rtl ? left : right); // after this column
                }
            }
            std::size_t best = 0;
            double bestDistance = 1.0e30;
            for (std::size_t i = 0; i < m_anchorEdges.size(); ++i) {
                const double distance = std::abs(m_anchorEdges[i] - xInTray);
                if (distance < bestDistance) { bestDistance = distance; best = i; }
            }
            const auto& anchor = m_anchors[best / 2];
            const bool before = (best % 2) == 0;
            const int column = anchor.first;
            Grid::SetColumn(m_marker, column);
            m_marker.HorizontalAlignment(before ? HorizontalAlignment::Left : HorizontalAlignment::Right);
            m_marker.Visibility(Visibility::Visible);
            m_previewColumn = before ? -(column + 2) : column;
            m_previewAnchor = logical
                ? (before ? -static_cast<int>(best / 2) - 2 : static_cast<int>(best / 2))
                : m_previewColumn;
            return logical ? m_previewAnchor : m_previewColumn;
        } catch (...) { return -1; }
    }

    int CommitPreview() noexcept {
        const int column = m_previewColumn;
        if (column < 0) { CancelPreview(); return -1; }
        m_marker.Visibility(winrt::Windows::UI::Xaml::Visibility::Collapsed);
        m_previewColumn = -1;
        m_previewAnchor = -1;
        m_anchorEdges.clear();
        return column;
    }

    int CommitAnchorPreview() noexcept {
        const int anchor = m_previewAnchor;
        if (anchor == -1 && m_previewColumn == -1) { CancelPreview(); return -1; }
        m_marker.Visibility(winrt::Windows::UI::Xaml::Visibility::Collapsed);
        m_previewColumn = m_previewAnchor = -1;
        m_anchorEdges.clear();
        return anchor;
    }

    void CancelPreview() noexcept {
        m_previewColumn = -1;
        m_previewAnchor = -1;
        m_anchorEdges.clear();
        try { if (m_marker) m_marker.Visibility(winrt::Windows::UI::Xaml::Visibility::Collapsed); } catch (...) { }
    }

    bool SetDesiredColumn(int column) noexcept {
        if (m_kind == Kind::StackPanel) {
            if (column < 0) column = m_defaultColumn;
            return MoveToStackAnchor(column);
        }
        if (m_kind != Kind::Grid) return false;
        if (column < 0) column = m_defaultColumn;
        return MoveToColumn(column);
    }

    bool SetDesiredAnchor(int anchor) noexcept {
        if (m_kind == Kind::StackPanel) {
            return MoveToStackAnchor(anchor < 0 ? m_defaultColumn : anchor);
        }
        if (m_kind != Kind::Grid) return false;
        if (anchor == -1) return MoveToColumn(m_defaultColumn);
        const bool before = anchor <= -2;
        const int index = before ? -anchor - 2 : anchor;
        if (index < 0 || index >= static_cast<int>(m_anchors.size())) return false;
        const int column = m_anchors[static_cast<std::size_t>(index)].first;
        return MoveToColumn(before ? -(column + 2) : column);
    }

    bool Remove() noexcept {
        try {
            if (m_kind == Kind::Grid && m_grid) {
                RemoveGridSlot();
                if (m_marker) { uint32_t i = 0; if (m_grid.Children().IndexOf(m_marker, i)) m_grid.Children().RemoveAt(i); }
            } else if (m_kind == Kind::StackPanel && m_stack) {
                uint32_t i = 0; if (m_stack.Children().IndexOf(m_item, i)) m_stack.Children().RemoveAt(i);
                if (m_marker && m_stack.Children().IndexOf(m_marker, i)) m_stack.Children().RemoveAt(i);
            }
            Reset();
            return true;
        } catch (...) { Reset(); return false; }
    }

    void Reset() noexcept {
        m_originalColumns.clear(); m_anchors.clear(); m_stackAnchors.clear(); m_anchorEdges.clear();
        m_grid = nullptr; m_stack = nullptr; m_item = nullptr; m_marker = nullptr;
        m_column = -1; m_anchorColumn = -1; m_previewColumn = -1; m_defaultColumn = -1; m_anchorRow = 0; m_stackCurrentAnchor = -1; m_kind = Kind::None;
    }

private:
    enum class Kind { None, Grid, StackPanel };
    static bool IsOwned(winrt::Windows::UI::Xaml::FrameworkElement const& element) noexcept {
        try { return wcsncmp(element.Name().c_str(), L"BatteryHub", 10) == 0; } catch (...) { return false; }
    }
    void CreateMarker() {
        using namespace winrt::Windows::UI::Xaml;
        using namespace winrt::Windows::UI::Xaml::Controls;
        using namespace winrt::Windows::UI::Xaml::Media;
        m_marker = Canvas();
        m_marker.Name(L"BatteryHubInsertionMarker");
        m_marker.Width(0.0);
        m_marker.Height(20.0);
        m_marker.VerticalAlignment(VerticalAlignment::Center);
        m_marker.IsHitTestVisible(false);
        m_markerLine = Border();
        m_markerLine.Background(SolidColorBrush(winrt::Windows::UI::Color{255, 0, 120, 215}));
        m_markerLine.Width(2.0);
        m_markerLine.Height(20.0);
        m_markerLine.VerticalAlignment(VerticalAlignment::Center);
        m_markerLine.IsHitTestVisible(false);
        Canvas::SetLeft(m_markerLine, -1.0);
        m_marker.Children().Append(m_markerLine);
        m_marker.Visibility(Visibility::Collapsed);
    }
    bool MoveToStackAnchor(int desired) noexcept {
        if (!m_stack || !m_item || !m_marker || desired < 0 || desired >= static_cast<int>(m_stackAnchors.size())) return false;
        if (desired == m_stackCurrentAnchor) return true;
        try {
            uint32_t index = 0;
            if (m_stack.Children().IndexOf(m_item, index)) m_stack.Children().RemoveAt(index);
            if (m_stack.Children().IndexOf(m_marker, index)) m_stack.Children().RemoveAt(index);
            if (m_stackAnchors[desired]) {
                if (!m_stack.Children().IndexOf(m_stackAnchors[desired], index)) return false;
                m_stack.Children().InsertAt(index + 1, m_item);
            } else {
                m_stack.Children().InsertAt(0, m_item);
            }
            m_stack.Children().Append(m_marker);
            m_stackCurrentAnchor = desired;
            m_anchorEdges.clear();
            return true;
        } catch (...) { return false; }
    }
    bool MoveToColumn(int desired) noexcept {
        using namespace winrt::Windows::UI::Xaml;
        using namespace winrt::Windows::UI::Xaml::Controls;
        if (!m_grid || !m_item) return false;
        if (desired < widget_bridge::kMinGridAnchor || desired >= static_cast<int>(widget_bridge::kMaxGridColumns)) return false;
        const bool before = desired <= -2;
        const int anchorColumn = before ? -desired - 2 : desired;
        auto found = std::find_if(m_anchors.begin(), m_anchors.end(), [anchorColumn](auto const& a) { return a.first == anchorColumn; });
        if (found == m_anchors.end()) return false;
        if (m_column >= 0 && desired == m_anchorColumn) return true;
        try {
            RemoveGridSlot();
            const int column = anchorColumn + (before ? 0 : 1);
            if (column > static_cast<int>(m_grid.ColumnDefinitions().Size())) return false;
            ColumnDefinition reserved;
            reserved.Width(GridLengthHelper::Auto());
            m_grid.ColumnDefinitions().InsertAt(column, reserved);
            m_column = column;
            for (auto const& saved : m_originalColumns)
                if (saved.first != m_item && saved.first != m_marker && saved.second >= column)
                    Grid::SetColumn(saved.first, saved.second + 1);
            Grid::SetColumn(m_item, column);
            uint32_t anchorIndex = 0;
            if (!m_grid.Children().IndexOf(found->second, anchorIndex)) throw winrt::hresult_error(E_FAIL);
            m_grid.Children().InsertAt(anchorIndex + (before ? 0u : 1u), m_item);
            m_anchorColumn = desired;
            return true;
        } catch (...) {
            RemoveGridSlot();
            m_anchorColumn = -1;
            return false;
        }
    }
    void RemoveGridSlot() noexcept {
        using namespace winrt::Windows::UI::Xaml::Controls;
        try {
            uint32_t index = 0;
            if (m_grid.Children().IndexOf(m_item, index)) m_grid.Children().RemoveAt(index);
            if (m_column >= 0 && m_column < static_cast<int>(m_grid.ColumnDefinitions().Size()))
                m_grid.ColumnDefinitions().RemoveAt(m_column);
            for (auto const& saved : m_originalColumns)
                if (saved.first && saved.first != m_item && saved.first != m_marker) Grid::SetColumn(saved.first, saved.second);
            m_column = -1;
        } catch (...) { }
    }

    Kind m_kind = Kind::None;
    int m_column = -1, m_anchorColumn = -1, m_previewColumn = -1, m_previewAnchor = -1, m_defaultColumn = -1, m_anchorRow = 0;
    winrt::Windows::UI::Xaml::Controls::Grid m_grid{ nullptr };
    winrt::Windows::UI::Xaml::Controls::StackPanel m_stack{ nullptr };
    winrt::Windows::UI::Xaml::FrameworkElement m_item{ nullptr };
    winrt::Windows::UI::Xaml::Controls::Canvas m_marker{ nullptr };
    winrt::Windows::UI::Xaml::Controls::Border m_markerLine{ nullptr };
    std::vector<std::pair<winrt::Windows::UI::Xaml::FrameworkElement, int>> m_originalColumns;
    std::vector<std::pair<int, winrt::Windows::UI::Xaml::FrameworkElement>> m_anchors;
    std::vector<winrt::Windows::UI::Xaml::FrameworkElement> m_stackAnchors;
    std::vector<double> m_anchorEdges;
    int m_stackCurrentAnchor = -1;
    bool m_stackRightToLeft = false;
};
