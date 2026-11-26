import AppKit
@_exported import HortusApple

@MainActor
extension DiagramExport {
    /// 原生保存面板；用户选定路径后才在后台生成并写入。
    static func presentSavePanel(format: DiagramExportFormat, request: DiagramExportRequest,
                                 window: NSWindow?, completion: ((URL?) -> Void)? = nil) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [format.contentType]
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false
        panel.nameFieldStringValue = defaultName(note: request.note, ordinal: request.ordinal,
                                                 format: format)
        let finish: @MainActor (NSApplication.ModalResponse) -> Void = { response in
            guard response == .OK, let url = panel.url else {
                completion?(nil)
                return
            }
            Task { @MainActor in
                do {
                    try await export(format, request: request, to: url)
                    completion?(url)
                } catch {
                    presentFailure(error, window: window)
                    completion?(nil)
                }
            }
        }
        if let window {
            panel.beginSheetModal(for: window) { response in
                MainActor.assumeIsolated { finish(response) }
            }
        } else {
            panel.begin { response in
                MainActor.assumeIsolated { finish(response) }
            }
        }
    }

    private static func presentFailure(_ error: Error, window: NSWindow?) {
        let alert = NSAlert()
        alert.messageText = DiagramExportStrings.exportFailed
        alert.informativeText = error.localizedDescription
        alert.alertStyle = .warning
        if let window {
            alert.beginSheetModal(for: window, completionHandler: nil)
        } else {
            alert.runModal()
        }
    }
}
