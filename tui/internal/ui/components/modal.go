package components

import (
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"github.com/stui/stui/pkg/theme"
)

type ConfirmDialog struct {
	title     string
	message   string
	onConfirm func()
	onCancel  func()
	cursor    int
	width     int
	height    int
	focused   bool
}

type ConfirmResultMsg struct {
	Confirmed bool
}

func NewConfirmDialog(title, message string, onConfirm, onCancel func()) *ConfirmDialog {
	return &ConfirmDialog{
		title:     title,
		message:   message,
		onConfirm: onConfirm,
		onCancel:  onCancel,
		cursor:    0,
		width:     40,
		height:    8,
		focused:   true,
	}
}

func (d *ConfirmDialog) Init() tea.Cmd {
	return nil
}

func (d *ConfirmDialog) Update(msg tea.Msg) (tea.Msg, tea.Cmd) {
	if !d.focused {
		return msg, nil
	}

	switch m := msg.(type) {
	case tea.KeyPressMsg:
		switch m.String() {
		case "left", "h":
			if d.cursor > 0 {
				d.cursor--
			}
		case "right", "l":
			if d.cursor < 1 {
				d.cursor++
			}
		case "enter":
			if d.cursor == 0 {
				if d.onConfirm != nil {
					d.onConfirm()
				}
				return ConfirmResultMsg{Confirmed: true}, nil
			} else {
				if d.onCancel != nil {
					d.onCancel()
				}
				return ConfirmResultMsg{Confirmed: false}, nil
			}
		case "esc", "q":
			if d.onCancel != nil {
				d.onCancel()
			}
			return ConfirmResultMsg{Confirmed: false}, nil
		}
	}
	return msg, nil
}

func (d *ConfirmDialog) View() tea.View {
	accent := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	dim := lipgloss.NewStyle().Foreground(theme.T.TextDim())
	text := lipgloss.NewStyle().Foreground(theme.T.Text())
	border := lipgloss.NewStyle().
		Foreground(theme.T.Border()).
		BorderStyle(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Accent()).
		Padding(1, 2)

	var sb strings.Builder

	sb.WriteString("\n")

	titleLen := len(d.title) + 4
	padding := (d.width - titleLen) / 2
	if padding < 0 {
		padding = 0
	}
	sb.WriteString(strings.Repeat(" ", padding) + accent.Render("┌─ "+d.title+" ─┐") + "\n")

	contentW := d.width - 4
	lines := wrapDialogText(d.message, contentW)
	for _, line := range lines {
		pad := (d.width - len(line) - 4) / 2
		if pad < 0 {
			pad = 0
		}
		sb.WriteString(strings.Repeat(" ", pad) + "│  " + text.Render(line) + strings.Repeat(" ", d.width-len(line)-4-pad) + "  │\n")
	}

	sb.WriteString(strings.Repeat(" ", padding) + accent.Render("└"+strings.Repeat("─", titleLen+4)+"┘") + "\n\n")

	yesBtn := "  Yes  "
	noBtn := "  No   "
	yesStyle := accent
	noStyle := text
	yesCursor := ""
	noCursor := ""

	if d.cursor == 0 {
		yesStyle = lipgloss.NewStyle().
			Foreground(theme.T.Bg()).
			Background(theme.T.Accent()).
			Bold(true)
		yesCursor = "▶"
	} else {
		noStyle = lipgloss.NewStyle().
			Foreground(theme.T.Bg()).
			Background(theme.T.Accent()).
			Bold(true)
		noCursor = "▶"
	}

	btnSpace := d.width - len(yesBtn) - len(noBtn) - 4
	leftPad := btnSpace / 2
	rightPad := btnSpace - leftPad

	sb.WriteString(strings.Repeat(" ", leftPad))
	sb.WriteString(yesCursor + yesStyle.Render(yesBtn))
	sb.WriteString(strings.Repeat(" ", rightPad))
	sb.WriteString(noCursor + noStyle.Render(noBtn))
	sb.WriteString("\n\n")

	hint := dim.Render("← → navigate · enter confirm · esc cancel")
	hintPad := (d.width - len(hint)) / 2
	if hintPad < 0 {
		hintPad = 0
	}
	sb.WriteString(strings.Repeat(" ", hintPad) + hint + "\n")

	_ = border
	return tea.NewView(sb.String())
}

func (d *ConfirmDialog) Focus() {
	d.focused = true
}

func (d *ConfirmDialog) Blur() {
	d.focused = false
}

func (d *ConfirmDialog) IsFocused() bool {
	return d.focused
}

// Dialog is an immutable value-type pop-up with a message and selectable
// buttons. Navigate with h/l or ←/→, confirm with enter, cancel with esc.
//
// Usage:
//
//	d := components.NewDialog("Add 'Song Title'?",
//	    []string{"Add to queue", "Replace queue", "Cancel"})
//
//	// In Update:
//	d, chosen, dismissed = d.Update(key)
//
//	// In View — overlay centered in the available area:
//	if dialogOpen {
//	    return lipgloss.Place(w, h, lipgloss.Center, lipgloss.Center, d.Render())
//	}
type Dialog struct {
	Message string
	Options []string
	Cursor  int
}

// NewDialog creates a Dialog with the cursor on the first option.
func NewDialog(message string, options []string) Dialog {
	return Dialog{Message: message, Options: options}
}

// Update handles a key string. Returns the updated dialog, the chosen option
// index (or -1 on esc/cancel), and whether the dialog was dismissed.
func (d Dialog) Update(key string) (out Dialog, chosen int, dismissed bool) {
	out = d
	switch key {
	case "h", "left":
		if out.Cursor > 0 {
			out.Cursor--
		}
	case "l", "right", "tab":
		if out.Cursor < len(out.Options)-1 {
			out.Cursor++
		}
	case "enter":
		return out, out.Cursor, true
	case "esc":
		return out, -1, true
	}
	return out, -1, false
}

// Render returns the styled dialog box string. Center it with lipgloss.Place.
func (d Dialog) Render() string {
	textStyle := lipgloss.NewStyle().Foreground(theme.T.Text())
	dimStyle := lipgloss.NewStyle().Foreground(theme.T.TextDim())

	// Message.
	const msgMaxW = 38
	wrapped := dialogWrapText(d.Message, msgMaxW)
	msgLines := make([]string, len(wrapped))
	for i, l := range wrapped {
		msgLines[i] = textStyle.Render(l)
	}
	msgBlock := strings.Join(msgLines, "\n")

	// Buttons: selected = solid accent fill; unselected = rounded border.
	var btnParts []string
	for i, label := range d.Options {
		if i == d.Cursor {
			btn := lipgloss.NewStyle().
				Background(theme.T.Accent()).
				Foreground(theme.T.Bg()).
				Bold(true).
				Padding(0, 2).
				Render(label)
			btnParts = append(btnParts, btn)
		} else {
			btn := lipgloss.NewStyle().
				Border(lipgloss.RoundedBorder()).
				BorderForeground(theme.T.Border()).
				Foreground(theme.T.TextDim()).
				Padding(0, 1).
				Render(label)
			btnParts = append(btnParts, btn)
		}
	}
	buttonRow := lipgloss.JoinHorizontal(lipgloss.Center, btnParts...)

	hint := dimStyle.Render("← → navigate · enter · esc cancel")

	inner := lipgloss.JoinVertical(lipgloss.Center,
		msgBlock,
		"",
		buttonRow,
		"",
		hint,
	)

	return lipgloss.NewStyle().
		Border(lipgloss.RoundedBorder()).
		BorderForeground(theme.T.Border()).
		Padding(1, 3).
		Render(inner)
}

// dialogWrapText wraps s into lines of at most maxW visible characters.
func dialogWrapText(s string, maxW int) []string {
	var lines []string
	words := strings.Fields(s)
	cur := ""
	for _, w := range words {
		if cur == "" {
			cur = w
		} else if len(cur)+1+len(w) <= maxW {
			cur += " " + w
		} else {
			lines = append(lines, cur)
			cur = w
		}
	}
	if cur != "" {
		lines = append(lines, cur)
	}
	if len(lines) == 0 {
		lines = []string{""}
	}
	return lines
}

func wrapDialogText(text string, maxWidth int) []string {
	var lines []string
	words := strings.Fields(text)
	currentLine := ""

	for _, word := range words {
		testLine := currentLine
		if testLine != "" {
			testLine += " "
		}
		testLine += word

		if len(testLine) > maxWidth && currentLine != "" {
			lines = append(lines, currentLine)
			currentLine = word
		} else {
			currentLine = testLine
		}
	}

	if currentLine != "" {
		lines = append(lines, currentLine)
	}

	return lines
}

type Modal struct {
	title    string
	content  string
	width    int
	height   int
	focused  bool
	children []ModalChild
}

type ModalChild interface {
	View() tea.View
	Update(tea.Msg) (tea.Msg, tea.Cmd)
	Init() tea.Cmd
}

func NewModal(title string, width, height int) *Modal {
	return &Modal{
		title:   title,
		width:   width,
		height:  height,
		focused: true,
	}
}

func (m *Modal) AddChild(child ModalChild) {
	m.children = append(m.children, child)
}

func (m *Modal) Init() tea.Cmd {
	return nil
}

func (m *Modal) Update(msg tea.Msg) (tea.Msg, tea.Cmd) {
	if !m.focused {
		return msg, nil
	}

	for _, child := range m.children {
		if updatedMsg, cmd := child.Update(msg); cmd != nil {
			return updatedMsg, cmd
		}
	}
	return msg, nil
}

func (m *Modal) View() tea.View {
	accent := lipgloss.NewStyle().Foreground(theme.T.Accent()).Bold(true)
	text := lipgloss.NewStyle().Foreground(theme.T.Text())

	var sb strings.Builder

	header := accent.Render("┌─ " + m.title + " ─┐")
	headerPad := (m.width - len(m.title) - 6) / 2
	sb.WriteString(strings.Repeat(" ", headerPad) + header + "\n")

	contentLines := strings.Split(m.content, "\n")
	for _, line := range contentLines {
		if len(line) > m.width-4 {
			line = line[:m.width-7] + "..."
		}
		pad := m.width - len(line) - 4
		sb.WriteString("│  " + text.Render(line) + strings.Repeat(" ", pad) + "  │\n")
	}

	footer := accent.Render("└" + strings.Repeat("─", len(m.title)+6) + "┘")
	sb.WriteString(strings.Repeat(" ", headerPad) + footer)

	return tea.NewView(sb.String())
}

func (m *Modal) Focus() {
	m.focused = true
}

func (m *Modal) Blur() {
	m.focused = false
}
