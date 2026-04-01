import ArgumentParser
import Foundation

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
