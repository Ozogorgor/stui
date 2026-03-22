package components

import (
	"sort"

	"charm.land/bubbles/v2/table"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

type Column = table.Column

type StyledTable struct {
	model   table.Model
	focused bool
}

func NewStyledTable(columns []Column) *StyledTable {
	t := table.New(
		table.WithColumns(columns),
		table.WithHeight(10),
	)

	t.SetStyles(table.Styles{
		Header:   tableHeaderStyle(),
		Cell:     tableCellStyle(),
		Selected: tableSelectedStyle(),
	})

	return &StyledTable{
		model:   t,
		focused: false,
	}
}

func tableHeaderStyle() lipgloss.Style {
	return lipgloss.NewStyle().
		Foreground(theme.T.TextMuted()).
		Bold(true).
		BorderStyle(lipgloss.NormalBorder()).
		BorderForeground(theme.T.Border()).
		BorderBottom(true)
}

func tableCellStyle() lipgloss.Style {
	return lipgloss.NewStyle().
		Foreground(theme.T.Text())
}

func tableSelectedStyle() lipgloss.Style {
	return lipgloss.NewStyle().
		Foreground(theme.T.Accent()).
		Bold(true)
}

func tableFocusedRowStyle() lipgloss.Style {
	return lipgloss.NewStyle().
		Background(theme.T.Surface()).
		Foreground(theme.T.Accent()).
		Bold(true)
}

func (s *StyledTable) Model() table.Model {
	return s.model
}

func (s *StyledTable) SetRows(rows [][]string) {
	tableRows := make([]table.Row, len(rows))
	for i := range rows {
		tableRows[i] = rows[i]
	}
	s.model.SetRows(tableRows)
}

func (s *StyledTable) SetHeight(h int) {
	s.model.SetHeight(h)
}

func (s *StyledTable) SetWidth(w int) {
	s.model.SetWidth(w)
}

func (s *StyledTable) Cursor() int {
	return s.model.Cursor()
}

func (s *StyledTable) SetCursor(n int) {
	s.model.SetCursor(n)
}

func (s *StyledTable) MoveUp(n int) {
	s.model.MoveUp(n)
}

func (s *StyledTable) MoveDown(n int) {
	s.model.MoveDown(n)
}

func (s *StyledTable) SelectedRow() []string {
	return s.model.SelectedRow()
}

func (s *StyledTable) SetFocused(focused bool) {
	s.focused = focused
	if focused {
		s.model.SetStyles(table.Styles{
			Header:   tableHeaderStyle(),
			Cell:     tableCellStyle(),
			Selected: tableFocusedRowStyle(),
		})
	} else {
		s.model.SetStyles(table.Styles{
			Header:   tableHeaderStyle(),
			Cell:     tableCellStyle(),
			Selected: tableSelectedStyle(),
		})
	}
}

func (s *StyledTable) Update(msg tea.Msg) (tea.Msg, tea.Cmd) {
	return s.model.Update(msg)
}

func (s *StyledTable) View() string {
	return s.model.View()
}

type SortableTable struct {
	StyledTable
	sortColumn int
	sortDesc   bool
	columns    []Column
	allRows    [][]string
}

func NewSortableTable(columns []Column) *SortableTable {
	t := NewStyledTable(columns)
	return &SortableTable{
		StyledTable: *t,
		columns:     columns,
		sortColumn:  -1,
		sortDesc:    true,
	}
}

func (s *SortableTable) SetData(rows [][]string) {
	s.allRows = rows
	s.Sort()
}

func (s *SortableTable) Sort() {
	if s.sortColumn < 0 || s.sortColumn >= len(s.columns) {
		s.SetRows(s.allRows)
		return
	}

	sorted := make([][]string, len(s.allRows))
	copy(sorted, s.allRows)

	col := s.sortColumn
	desc := s.sortDesc

	sort.Slice(sorted, func(i, j int) bool {
		a, b := sorted[i][col], sorted[j][col]
		if desc {
			a, b = b, a
		}
		return a < b
	})

	s.SetRows(sorted)
}

func (s *SortableTable) SetSortColumn(col int) {
	if s.sortColumn == col {
		s.sortDesc = !s.sortDesc
	} else {
		s.sortColumn = col
		s.sortDesc = true
	}
	s.Sort()
}

func (s *SortableTable) SortColumn() int {
	return s.sortColumn
}

func (s *SortableTable) SortDescending() bool {
	return s.sortDesc
}

type SelectableTable struct {
	StyledTable
	selected map[int]bool
	allRows  [][]string
}

func NewSelectableTable(columns []Column) *SelectableTable {
	t := NewStyledTable(columns)
	return &SelectableTable{
		StyledTable: *t,
		selected:    make(map[int]bool),
	}
}

func (s *SelectableTable) ToggleSelect(index int) {
	if s.selected[index] {
		delete(s.selected, index)
	} else {
		s.selected[index] = true
	}
}

func (s *SelectableTable) IsSelected(index int) bool {
	return s.selected[index]
}

func (s *SelectableTable) SelectedIndices() []int {
	var indices []int
	for i := range s.selected {
		indices = append(indices, i)
	}
	return indices
}

func (s *SelectableTable) ClearSelection() {
	s.selected = make(map[int]bool)
}

func (s *SelectableTable) SelectAll() {
	for i := 0; i < len(s.allRows); i++ {
		s.selected[i] = true
	}
}

func (s *SelectableTable) AllRows() [][]string {
	return s.allRows
}

func (s *SelectableTable) SetData(rows [][]string) {
	s.allRows = rows
	s.SetRows(rows)
}

type FilterableTable struct {
	StyledTable
	allRows  [][]string
	filter   string
	filtered [][]string
}

func NewFilterableTable(columns []Column) *FilterableTable {
	t := NewStyledTable(columns)
	return &FilterableTable{
		StyledTable: *t,
		filter:      "",
	}
}

func (f *FilterableTable) SetData(rows [][]string) {
	f.allRows = rows
	f.ApplyFilter()
}

func (f *FilterableTable) SetFilter(filter string) {
	f.filter = filter
	f.ApplyFilter()
}

func (f *FilterableTable) ApplyFilter() {
	if f.filter == "" {
		f.filtered = f.allRows
	} else {
		f.filtered = nil
		filterLower := toLower(f.filter)
		for _, row := range f.allRows {
			for _, cell := range row {
				if contains(toLower(cell), filterLower) {
					f.filtered = append(f.filtered, row)
					break
				}
			}
		}
	}
	f.SetRows(f.filtered)
}

func (f *FilterableTable) Filter() string {
	return f.filter
}

func (f *FilterableTable) MatchCount() int {
	return len(f.filtered)
}

func (f *FilterableTable) TotalCount() int {
	return len(f.allRows)
}

func toLower(s string) string {
	result := make([]byte, len(s))
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 'A' && c <= 'Z' {
			c += 'a' - 'A'
		}
		result[i] = c
	}
	return string(result)
}

func contains(s, substr string) bool {
	if len(substr) == 0 {
		return true
	}
	if len(substr) > len(s) {
		return false
	}
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
