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
