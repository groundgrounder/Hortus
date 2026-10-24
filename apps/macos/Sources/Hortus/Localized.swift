import Foundation
import HortusCore

enum L10n {
    static func string(_ key: String, _ arguments: CVarArg...) -> String {
        let format = NSLocalizedString(key, bundle: .main, comment: "")
        guard !arguments.isEmpty else { return format }
        return String(format: format, locale: Locale.current, arguments: arguments)
    }

    static func error(_ error: Error) -> String {
        if let error = error as? CoreError {
            switch error {
            case let .Failure(kind): return errorKey(kind)
            }
        }
        return string("Operation failed")
    }

    static func errorKey(_ key: String) -> String {
        switch key {
        case "document.missing", "io.not_found":
            return string("The file was deleted or moved")
        case "readonly.no_write_permission", "note.not_writable", "note.directory_not_writable":
            return string("This file has no write permission and is read-only")
        case "io.permission_denied":
            return string("Permission denied")
        case "readonly.invalid_utf8":
            return string("This file is not UTF-8 and is read-only")
        case "document.conflict":
            return string("The file changed on disk and your edits are unsaved")
        case "document.recovery":
            return string("Recovery draft available")
        case "document.recovery_read_only":
            return string("Read-only recovery draft kept")
        case "config.vault_contains_state":
            return string("The selected folder contains Hortus configuration or cache files")
        case "vault.outside_root", "vault.invalid_path", "note.invalid_path":
            return string("The selected note path is invalid")
        case "config.home_missing", "config.path_not_absolute", "config.path_invalid":
            return string("Hortus configuration is unavailable")
        case "config.migration_failed":
            return string("Unable to migrate Hortus data; the old data was kept")
        case "config.layout_missing_region", "config.layout_invalid_size", "config.layout_invalid_group",
             "config.layout_invalid_view", "config.layout_incomplete":
            return string("Saved layout is invalid")
        case "config.font_size_invalid":
            return string("Font size must be between 9 and 40 points")
        case "note.symlink":
            return string("Symbolic links cannot be edited")
        case "note.temp_inside_vault":
            return string("Temporary save files cannot be inside a note folder")
        case "note.missing_parent":
            return string("The note folder is unavailable")
        case "note.different_volume":
            return string("The temporary save folder must be on the same volume as the note")
        case "note.recovery_path_mismatch":
            return string("The recovery draft does not match this note")
        case "document.read_only":
            return string("This file has no write permission and is read-only")
        case "document.conflict_copy_exhausted":
            return string("Unable to create a conflict copy")
        case "vault.invalid_utf8_path":
            return string("The selected note path is invalid")
        case "vault.select_folder":
            return string("Choose a folder")
        case "data.invalid":
            return string("Saved data could not be read")
        case "io.invalid_input":
            return string("The data is invalid")
        case "io.already_exists":
            return string("A file already exists at the destination")
        case "document.save_failed":
            return string("Save failed")
        case "document.range_invalid", "io.failure":
            return string("Operation failed")
        default:
            return string("Operation failed")
        }
    }

    static func documentMessage(issue: DocumentIssue, readOnlyReason: String?, raw: String) -> String {
        if issue == .none, let readOnlyReason {
            switch readOnlyReason {
            case "readonly.invalid_utf8": return string("This file is not UTF-8 and is read-only")
            case "readonly.no_write_permission": return string("This file has no write permission and is read-only")
            default: return errorKey(readOnlyReason)
            }
        }
        switch raw {
        case "document.missing": return string("The file was deleted or moved")
        case "document.conflict": return string("The file changed on disk and your edits are unsaved")
        case "document.recovery": return string("Recovery draft available")
        case "document.save_failed": return string("Save failed")
        case "document.recovery_read_only": return string("Read-only recovery draft kept")
        default: return raw.isEmpty ? string("Operation failed") : errorKey(raw)
        }
    }
}
