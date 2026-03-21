import ArgumentParser
import Foundation

// MARK: - Helper

func printJSON(_ value: Any) {
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
       let string = String(data: data, encoding: .utf8) {
        print(string)
    } else {
        fputs("Failed to serialize JSON\n", stderr)
        Foundation.exit(1)
    }
}

// MARK: - Root Command

@main
struct AlephBridge: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "aleph-bridge",
        abstract: "macOS native API bridge for Aleph",
        subcommands: [Notes.self, Calendar.self, Reminders.self, Contacts.self, System.self]
    )
}

// MARK: - Notes

extension AlephBridge {
    struct Notes: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Apple Notes operations",
            subcommands: [List.self, Get.self, Create.self, Update.self, Delete.self, Folders.self]
        )

        struct List: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List notes")

            @Option(name: .long, help: "Filter by folder name")
            var folder: String?

            func run() {
                printJSON(["notes": [], "folder": folder as Any])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get a note by ID")

            @Argument(help: "Note ID")
            var id: String

            func run() {
                printJSON(["id": id, "title": "", "body": "", "folder": ""])
            }
        }

        struct Create: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Create a new note")

            @Option(name: .long, help: "Note title")
            var title: String = ""

            @Option(name: .long, help: "Note body (plain text or HTML)")
            var body: String = ""

            @Option(name: .long, help: "Folder name")
            var folder: String?

            func run() {
                printJSON(["id": "", "title": title, "body": body, "folder": folder as Any])
            }
        }

        struct Update: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Update an existing note")

            @Argument(help: "Note ID")
            var id: String

            @Option(name: .long, help: "New title")
            var title: String?

            @Option(name: .long, help: "New body")
            var body: String?

            func run() {
                printJSON(["id": id, "title": title as Any, "body": body as Any])
            }
        }

        struct Delete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Delete a note")

            @Argument(help: "Note ID")
            var id: String

            func run() {
                printJSON(["deleted": true, "id": id])
            }
        }

        struct Folders: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all note folders")

            func run() {
                printJSON(["folders": []])
            }
        }
    }
}

// MARK: - Calendar

extension AlephBridge {
    struct Calendar: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Apple Calendar operations",
            subcommands: [Events.self, Get.self, Create.self, Update.self, Delete.self, Calendars.self]
        )

        struct Events: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List calendar events")

            @Option(name: .long, help: "Start date (ISO 8601)")
            var from: String?

            @Option(name: .long, help: "End date (ISO 8601)")
            var to: String?

            @Option(name: .long, help: "Calendar ID filter")
            var calendarId: String?

            func run() {
                printJSON(["events": [], "from": from as Any, "to": to as Any, "calendarId": calendarId as Any])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get an event by ID")

            @Argument(help: "Event ID")
            var id: String

            func run() {
                printJSON(["id": id, "title": "", "start": "", "end": ""])
            }
        }

        struct Create: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Create a calendar event")

            @Option(name: .long, help: "Event title")
            var title: String = ""

            @Option(name: .long, help: "Start datetime (ISO 8601)")
            var start: String = ""

            @Option(name: .long, help: "End datetime (ISO 8601)")
            var end: String = ""

            @Option(name: .long, help: "Calendar ID")
            var calendarId: String?

            @Option(name: .long, help: "Location")
            var location: String?

            @Option(name: .long, help: "Notes / description")
            var notes: String?

            @Flag(name: .long, help: "All-day event")
            var allDay: Bool = false

            func run() {
                printJSON([
                    "id": "",
                    "title": title,
                    "start": start,
                    "end": end,
                    "calendarId": calendarId as Any,
                    "location": location as Any,
                    "notes": notes as Any,
                    "allDay": allDay,
                ])
            }
        }

        struct Update: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Update a calendar event")

            @Argument(help: "Event ID")
            var id: String

            @Option(name: .long, help: "New title")
            var title: String?

            @Option(name: .long, help: "New start datetime (ISO 8601)")
            var start: String?

            @Option(name: .long, help: "New end datetime (ISO 8601)")
            var end: String?

            @Option(name: .long, help: "New location")
            var location: String?

            @Option(name: .long, help: "New notes")
            var notes: String?

            func run() {
                printJSON([
                    "id": id,
                    "title": title as Any,
                    "start": start as Any,
                    "end": end as Any,
                    "location": location as Any,
                    "notes": notes as Any,
                ])
            }
        }

        struct Delete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Delete a calendar event")

            @Argument(help: "Event ID")
            var id: String

            func run() {
                printJSON(["deleted": true, "id": id])
            }
        }

        struct Calendars: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all calendars")

            func run() {
                printJSON(["calendars": []])
            }
        }
    }
}

// MARK: - Reminders

extension AlephBridge {
    struct Reminders: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Apple Reminders operations",
            subcommands: [List.self, Get.self, Create.self, Complete.self, Delete.self, Lists.self]
        )

        struct List: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List reminders")

            @Option(name: .long, help: "Reminder list ID")
            var listId: String?

            @Flag(name: .long, help: "Include completed reminders")
            var includeCompleted: Bool = false

            func run() {
                printJSON(["reminders": [], "listId": listId as Any, "includeCompleted": includeCompleted])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get a reminder by ID")

            @Argument(help: "Reminder ID")
            var id: String

            func run() {
                printJSON(["id": id, "title": "", "completed": false])
            }
        }

        struct Create: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Create a reminder")

            @Option(name: .long, help: "Reminder title")
            var title: String = ""

            @Option(name: .long, help: "Reminder list ID")
            var listId: String?

            @Option(name: .long, help: "Due date (ISO 8601)")
            var dueDate: String?

            @Option(name: .long, help: "Priority (0=none, 1=high, 5=medium, 9=low)")
            var priority: Int?

            @Option(name: .long, help: "Notes")
            var notes: String?

            func run() {
                printJSON([
                    "id": "",
                    "title": title,
                    "listId": listId as Any,
                    "dueDate": dueDate as Any,
                    "priority": priority as Any,
                    "notes": notes as Any,
                ])
            }
        }

        struct Complete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Mark a reminder as completed or not")

            @Argument(help: "Reminder ID")
            var id: String

            @Flag(name: .long, help: "Mark as completed (default: true)")
            var completed: Bool = false

            func run() {
                // When --completed flag is absent, default to marking complete
                let isCompleted = completed ? completed : true
                printJSON(["id": id, "completed": isCompleted])
            }
        }

        struct Delete: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Delete a reminder")

            @Argument(help: "Reminder ID")
            var id: String

            func run() {
                printJSON(["deleted": true, "id": id])
            }
        }

        struct Lists: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all reminder lists")

            func run() {
                printJSON(["lists": []])
            }
        }
    }
}

// MARK: - Contacts

extension AlephBridge {
    struct Contacts: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "Apple Contacts operations",
            subcommands: [Search.self, Get.self, Groups.self]
        )

        struct Search: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Search contacts")

            @Option(name: .long, help: "Search query (name, email, phone)")
            var query: String = ""

            func run() {
                printJSON(["contacts": [], "query": query])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get a contact by ID")

            @Argument(help: "Contact ID")
            var id: String

            func run() {
                printJSON(["id": id, "firstName": "", "lastName": "", "emails": [], "phones": []])
            }
        }

        struct Groups: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all contact groups")

            func run() {
                printJSON(["groups": []])
            }
        }
    }
}

// MARK: - System

extension AlephBridge {
    struct System: ParsableCommand {
        static let configuration = CommandConfiguration(
            abstract: "macOS system information",
            subcommands: [Info.self]
        )

        struct Info: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Print system information")

            func run() {
                let info = ProcessInfo.processInfo
                let osVersion = info.operatingSystemVersionString
                let hostname = info.hostName
                let username = NSUserName()

                printJSON([
                    "os_version": osVersion,
                    "hostname": hostname,
                    "username": username,
                ])
            }
        }
    }
}
