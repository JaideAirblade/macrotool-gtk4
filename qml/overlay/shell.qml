import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick

ShellRoot {
    id: root

    property var state: ({
        updatedAt: 0,
        enabled: false,
        activeGame: "",
        activeClass: "",
        activeSpec: "",
        macros: [],
        buffs: [],
        gameActive: false,
        gameAlive: false,
        gamePid: 0,
        overlayPosition: "top-left"
    })
    property bool stateLoaded: false
    // Layer-shell surfaces are global in Mango, so only map this one while
    // the tracked game client itself is visible on the currently active tag.
    property bool gameVisibleOnActiveTag: false
    readonly property string statePath: Quickshell.env("MACROTOOL_OVERLAY_STATE") || ""

    function updateGameTagVisibility(source) {
        try {
            const clients = JSON.parse(source).clients || [];
            const gamePid = Number(root.state.gamePid || 0);
            gameVisibleOnActiveTag = gamePid > 0 && clients.some(client =>
                Number(client.pid) === gamePid && client.is_visible === true);
        } catch (error) {
            // Keep the last confirmed state while an IPC request is in flight.
            // Clearing it before every successful reply caused visible flicker.
        }
    }

    Process {
        id: gameClientProbe
        command: ["mmsg", "get", "all-clients"]
        running: true
        stdout: StdioCollector {
            onStreamFinished: root.updateGameTagVisibility(this.text)
        }
    }

    Timer {
        interval: 250
        running: true
        repeat: true
        onTriggered: gameClientProbe.running = true
    }

    function readState() {
        if (!stateFile.loaded)
            return;
        try {
            const next = JSON.parse(stateFile.text());
            if (next && next.updatedAt) {
                state = next;
                stateLoaded = true;
            }
        } catch (error) {
            // The writer uses atomic replacement, but retaining the previous
            // valid state also protects the overlay from a partial read.
        }
    }

    FileView {
        id: stateFile
        path: root.statePath
        blockLoading: true
        printErrors: false
        watchChanges: true
        onFileChanged: reload()
        onLoaded: root.readState()
        onTextChanged: root.readState()
    }

    Timer {
        interval: 100
        running: true
        repeat: true
        onTriggered: stateFile.reload()
    }

    // If Macrotool is killed in a way that bypasses cleanup, its heartbeat
    // stops. Quitting here prevents a detached/zombie overlay.
    Timer {
        interval: 1000
        running: root.stateLoaded
        repeat: true
        onTriggered: {
            if (Date.now() - Number(root.state.updatedAt || 0) > 3000)
                Qt.quit();
        }
    }

    SystemPalette {
        id: palette
        colorGroup: SystemPalette.Active
    }

    // Prefer colors resolved by Macrotool's realized GTK window. The Qt
    // SystemPalette remains the portable fallback.
    readonly property var gtkTheme: root.state.theme || ({})
    readonly property color windowColor: gtkTheme.window || palette.window
    readonly property color windowTextColor: gtkTheme.windowText || palette.windowText
    readonly property color highlightColor: gtkTheme.highlight || palette.highlight
    readonly property color highlightedTextColor: gtkTheme.highlightedText || palette.highlightedText
    readonly property color midColor: gtkTheme.mid || palette.mid
    readonly property string overlayPosition: root.state.overlayPosition || "top-left"

    PanelWindow {
        id: overlayWindow
        visible: root.stateLoaded && root.overlayPosition !== "hidden" && root.gameVisibleOnActiveTag
        implicitWidth: card.implicitWidth
        implicitHeight: card.implicitHeight
        color: "transparent"
        exclusionMode: ExclusionMode.Ignore
        aboveWindows: true
        focusable: false
        anchors {
            top: root.overlayPosition === "top-left" || root.overlayPosition === "top-right"
            bottom: root.overlayPosition === "bottom-left" || root.overlayPosition === "bottom-right"
            left: root.overlayPosition === "top-left" || root.overlayPosition === "bottom-left"
            right: root.overlayPosition === "top-right" || root.overlayPosition === "bottom-right"
        }
        margins {
            top: 10
            left: 10
            bottom: 10
            right: 10
        }
        mask: Region {}

        WlrLayershell.layer: WlrLayer.Overlay
        WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
        WlrLayershell.namespace: "macrotool-overlay"

        Rectangle {
            id: card
            width: overlayWindow.width
            implicitWidth: Math.max(body.implicitWidth, profileHeader.implicitWidth) + 24
            implicitHeight: body.implicitHeight + 24
            radius: 12
            color: Qt.rgba(root.windowColor.r, root.windowColor.g, root.windowColor.b, 0.90)
            border.width: 1
            border.color: Qt.rgba(root.windowTextColor.r, root.windowTextColor.g, root.windowTextColor.b, 0.18)

            Column {
                id: body
                x: 12
                y: 12
                width: parent.width - 24
                spacing: 7

                Item {
                    id: profileHeader
                    width: parent.width
                    height: profileRow.implicitHeight
                    implicitWidth: statusIndicator.implicitWidth + profileRow.spacing + profileTitle.implicitWidth
                    implicitHeight: profileRow.implicitHeight

                    Row {
                        id: profileRow
                        width: parent.width
                        height: implicitHeight
                        spacing: 8

                        Text {
                            id: statusIndicator
                            text: root.state.enabled ? "●" : "○"
                            color: root.state.enabled ? root.highlightColor : root.midColor
                            font.pixelSize: 14
                        }

                        Text {
                            id: profileTitle
                            width: parent.width - statusIndicator.width - parent.spacing
                            text: {
                                const parts = [root.state.activeGame, root.state.activeClass, root.state.activeSpec]
                                    .filter(part => part && part.length > 0);
                                return parts.length > 0 ? parts.join(" / ") : "No game selected";
                            }
                            color: root.windowTextColor
                            font.bold: true
                            font.pixelSize: 13
                            elide: Text.ElideRight
                        }
                    }
                }

                Text {
                    visible: !root.state.enabled
                    text: "Macros disabled (toggle key)"
                    color: root.windowTextColor
                    opacity: 0.65
                    font.pixelSize: 10
                }

                Text {
                    visible: root.state.enabled && (root.state.macros || []).length > 0
                    text: "MACROS"
                    color: root.windowTextColor
                    opacity: 0.55
                    font.bold: true
                    font.pixelSize: 9
                    font.letterSpacing: 1
                }

                Repeater {
                    model: root.state.enabled ? (root.state.macros || []) : []
                    delegate: Row {
                        required property var modelData
                        width: body.width
                        spacing: 7

                        Rectangle {
                            width: Math.max(42, hotkeyText.implicitWidth + 12)
                            implicitWidth: Math.max(42, hotkeyText.implicitWidth + 12)
                            height: 20
                            radius: 4
                            color: modelData.running
                                ? root.highlightColor
                                : Qt.rgba(root.windowTextColor.r, root.windowTextColor.g, root.windowTextColor.b, 0.12)

                            Text {
                                id: hotkeyText
                                anchors.centerIn: parent
                                text: modelData.hotkey || "—"
                                color: modelData.running ? root.highlightedTextColor : root.windowTextColor
                                opacity: modelData.running ? 1 : 0.72
                                font.bold: true
                                font.pixelSize: 10
                            }
                        }

                        Text {
                            width: body.width - 74
                            anchors.verticalCenter: parent.verticalCenter
                            text: modelData.name
                            color: root.windowTextColor
                            font.pixelSize: 11
                            elide: Text.ElideRight
                        }

                        Text {
                            visible: modelData.running
                            anchors.verticalCenter: parent.verticalCenter
                            text: "▶"
                            color: root.highlightColor
                            font.pixelSize: 10
                        }
                    }
                }

                Text {
                    visible: root.state.enabled && (root.state.buffs || []).length > 0
                    text: "BUFFS"
                    color: root.windowTextColor
                    opacity: 0.55
                    font.bold: true
                    font.pixelSize: 9
                    font.letterSpacing: 1
                }

                Repeater {
                    model: root.state.enabled ? (root.state.buffs || []) : []
                    delegate: Row {
                        required property var modelData
                        width: body.width
                        spacing: 6

                        Text {
                            width: 72
                            text: modelData.name
                            color: root.windowTextColor
                            font.pixelSize: 10
                            elide: Text.ElideRight
                        }

                        Rectangle {
                            width: body.width - 116
                            implicitWidth: 80
                            height: 6
                            anchors.verticalCenter: parent.verticalCenter
                            radius: 3
                            color: Qt.rgba(root.windowTextColor.r, root.windowTextColor.g, root.windowTextColor.b, 0.12)

                            Rectangle {
                                width: parent.width * Math.max(0, Math.min(1, modelData.fraction))
                                height: parent.height
                                radius: parent.radius
                                color: root.highlightColor
                            }
                        }

                        Text {
                            width: 32
                            text: (modelData.remainingMs / 1000).toFixed(1) + "s"
                            color: root.windowTextColor
                            opacity: 0.65
                            font.pixelSize: 10
                            horizontalAlignment: Text.AlignRight
                        }
                    }
                }

                Text {
                    visible: root.state.activeGame && !root.state.gameActive
                    text: root.state.gameAlive ? "Game not focused (background)" : "Game not running"
                    color: root.windowTextColor
                    opacity: 0.68
                    font.pixelSize: 10
                }
            }
        }
    }
}
