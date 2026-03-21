import ArgumentParser
import Contacts
import Foundation

// MARK: - Contacts (Contacts.framework)

private let contactStore = CNContactStore()

/// Request contacts access synchronously.
private func requestContactsAccess() {
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false

    contactStore.requestAccess(for: .contacts) { g, error in
        granted = g
        if let error = error {
            fputs("Contacts access error: \(error.localizedDescription)\n", stderr)
        }
        semaphore.signal()
    }
    semaphore.wait()
    if !granted {
        printError("Contacts access denied. Grant access in System Settings > Privacy & Security > Contacts.")
    }
}

/// Keys needed for summary (search results).
private let summaryKeys: [CNKeyDescriptor] = [
    CNContactIdentifierKey as CNKeyDescriptor,
    CNContactGivenNameKey as CNKeyDescriptor,
    CNContactFamilyNameKey as CNKeyDescriptor,
    CNContactEmailAddressesKey as CNKeyDescriptor,
    CNContactPhoneNumbersKey as CNKeyDescriptor,
]

/// Keys needed for full detail.
private let detailKeys: [CNKeyDescriptor] = [
    CNContactIdentifierKey as CNKeyDescriptor,
    CNContactGivenNameKey as CNKeyDescriptor,
    CNContactFamilyNameKey as CNKeyDescriptor,
    CNContactEmailAddressesKey as CNKeyDescriptor,
    CNContactPhoneNumbersKey as CNKeyDescriptor,
    CNContactPostalAddressesKey as CNKeyDescriptor,
    CNContactOrganizationNameKey as CNKeyDescriptor,
    CNContactJobTitleKey as CNKeyDescriptor,
    CNContactBirthdayKey as CNKeyDescriptor,
    CNContactNoteKey as CNKeyDescriptor,
    CNContactFormatter.descriptorForRequiredKeys(for: .fullName),
]

private func fullName(_ contact: CNContact) -> String {
    let name = CNContactFormatter.string(from: contact, style: .fullName)
    return name ?? "\(contact.givenName) \(contact.familyName)".trimmingCharacters(in: .whitespaces)
}

private func labelString(_ label: String?) -> String {
    guard let label = label else { return "other" }
    // CNLabeledValue labels are like "_$!<Home>!$_" — use localized version
    return CNLabeledValue<NSString>.localizedString(forLabel: label).lowercased()
}

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
                requestContactsAccess()

                let request = CNContactFetchRequest(keysToFetch: summaryKeys)
                if !query.isEmpty {
                    request.predicate = CNContact.predicateForContacts(matchingName: query)
                }

                var contacts: [[String: Any]] = []
                do {
                    try contactStore.enumerateContacts(with: request) { contact, _ in
                        var dict: [String: Any] = [
                            "id": contact.identifier,
                            "name": fullName(contact),
                        ]
                        if let email = contact.emailAddresses.first {
                            dict["email"] = email.value as String
                        }
                        if let phone = contact.phoneNumbers.first {
                            dict["phone"] = phone.value.stringValue
                        }
                        contacts.append(dict)
                    }
                } catch {
                    printError("Failed to search contacts: \(error.localizedDescription)")
                }

                printJSON(["contacts": contacts])
            }
        }

        struct Get: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "Get a contact by ID")

            @Option(name: .long, help: "Contact ID")
            var id: String

            func run() {
                requestContactsAccess()

                do {
                    let contacts = try contactStore.unifiedContacts(
                        matching: CNContact.predicateForContacts(withIdentifiers: [id]),
                        keysToFetch: detailKeys
                    )
                    guard let contact = contacts.first else {
                        printError("Contact not found: \(id)")
                    }

                    let emails: [[String: String]] = contact.emailAddresses.map { lv in
                        ["label": labelString(lv.label), "value": lv.value as String]
                    }
                    let phones: [[String: String]] = contact.phoneNumbers.map { lv in
                        ["label": labelString(lv.label), "value": lv.value.stringValue]
                    }
                    let addresses: [[String: String]] = contact.postalAddresses.map { lv in
                        let addr = lv.value
                        let formatted = CNPostalAddressFormatter.string(from: addr, style: .mailingAddress)
                        return ["label": labelString(lv.label), "value": formatted]
                    }

                    var dict: [String: Any] = [
                        "id": contact.identifier,
                        "name": fullName(contact),
                        "emails": emails,
                        "phones": phones,
                        "addresses": addresses,
                    ]
                    if !contact.organizationName.isEmpty {
                        dict["organization"] = contact.organizationName
                    }
                    if !contact.jobTitle.isEmpty {
                        dict["job_title"] = contact.jobTitle
                    }
                    if let birthday = contact.birthday,
                       let date = Foundation.Calendar.current.date(from: birthday) {
                        let fmt = iso8601Formatter()
                        dict["birthday"] = fmt.string(from: date)
                    }
                    if contact.isKeyAvailable(CNContactNoteKey) && !contact.note.isEmpty {
                        dict["notes"] = contact.note
                    }

                    printJSON(dict)
                } catch {
                    printError("Failed to get contact: \(error.localizedDescription)")
                }
            }
        }

        struct Groups: ParsableCommand {
            static let configuration = CommandConfiguration(abstract: "List all contact groups")

            func run() {
                requestContactsAccess()

                do {
                    let groups = try contactStore.groups(matching: nil)
                    let results: [[String: Any]] = try groups.map { group in
                        let members = try contactStore.unifiedContacts(
                            matching: CNContact.predicateForContactsInGroup(withIdentifier: group.identifier),
                            keysToFetch: [CNContactIdentifierKey as CNKeyDescriptor]
                        )
                        return [
                            "id": group.identifier,
                            "name": group.name,
                            "count": members.count,
                        ]
                    }
                    printJSON(["groups": results])
                } catch {
                    printError("Failed to list groups: \(error.localizedDescription)")
                }
            }
        }
    }
}
