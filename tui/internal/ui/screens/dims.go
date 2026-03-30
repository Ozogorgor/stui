package screens

import tea "charm.land/bubbletea/v2"

// Dims is embedded by every screen to hold its terminal dimensions.
// Embedding removes the need to redeclare width/height in each struct
// and reduces the WindowSizeMsg handler to a single method call.
type Dims struct {
	width  int
	height int
}

// setWindowSize updates the stored dimensions from a resize event.
func (d *Dims) setWindowSize(msg tea.WindowSizeMsg) {
	d.width = msg.Width
	d.height = msg.Height
}
