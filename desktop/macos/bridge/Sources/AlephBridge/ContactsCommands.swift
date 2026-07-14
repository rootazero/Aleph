import Contacts
import Foundation

// MARK: - Contacts (Contacts.framework)

private let contactStore = CNContactStore()

/// Request contacts access synchronously.
///
/// Safe only off the main thread — the RPC entry point parks the main thread on
/// a semaphore, and PIM calls run on the serial PIM queue.
private func requireContactsAccess() throws {
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
    guard granted else {
        throw RpcError(
            code: -32001,
            message: "permission denied: contacts",
            data: try encodeCodable(Perm.guide(.contacts))
        )
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

/// Render a birthday as the `chrono::NaiveDate` wire format the Rust side parses
/// ("yyyy-MM-dd"). Contacts allows a year-less birthday, which `NaiveDate` cannot
/// represent — those are reported as absent rather than guessed.
func contactBirthdayString(_ components: DateComponents?) -> String? {
    guard let components = components,
          let year = components.year,
          let month = components.month,
          let day = components.day
    else { return nil }
    return String(format: "%04d-%02d-%02d", year, month, day)
}

/// Apple Contacts operations. Synchronous and blocking; call from the serial PIM
/// queue (see `PimHandlers.swift`).
enum ContactsCommands {
    static func search(query: String) throws -> ContactsSearchResult {
        try requireContactsAccess()

        let request = CNContactFetchRequest(keysToFetch: summaryKeys)
        if !query.isEmpty {
            request.predicate = CNContact.predicateForContacts(matchingName: query)
        }

        var contacts: [Contact] = []
        do {
            try contactStore.enumerateContacts(with: request) { contact, _ in
                contacts.append(
                    Contact(
                        id: contact.identifier,
                        name: fullName(contact),
                        email: contact.emailAddresses.first.map { $0.value as String },
                        phone: contact.phoneNumbers.first.map { $0.value.stringValue }
                    )
                )
            }
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.contacts.search: \(error.localizedDescription)",
                data: nil
            )
        }

        return ContactsSearchResult(contacts: contacts)
    }

    static func get(id: String) throws -> ContactDetail {
        try requireContactsAccess()

        let contacts: [CNContact]
        do {
            contacts = try contactStore.unifiedContacts(
                matching: CNContact.predicateForContacts(withIdentifiers: [id]),
                keysToFetch: detailKeys
            )
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.contacts.get: \(error.localizedDescription)",
                data: nil
            )
        }

        guard let contact = contacts.first else {
            throw RpcError(
                code: -32602,
                message: "pim.contacts.get: contact not found: \(id)",
                data: nil
            )
        }

        let emails = contact.emailAddresses.map {
            LabeledValue(label: labelString($0.label), value: $0.value as String)
        }
        let phones = contact.phoneNumbers.map {
            LabeledValue(label: labelString($0.label), value: $0.value.stringValue)
        }
        let addresses = contact.postalAddresses.map {
            LabeledValue(
                label: labelString($0.label),
                value: CNPostalAddressFormatter.string(from: $0.value, style: .mailingAddress)
            )
        }

        // The note key needs a special entitlement; without it the value is not
        // fetched at all, so ask before reading.
        let notes = contact.isKeyAvailable(CNContactNoteKey) && !contact.note.isEmpty
            ? contact.note
            : nil

        return ContactDetail(
            id: contact.identifier,
            name: fullName(contact),
            emails: emails,
            phones: phones,
            addresses: addresses,
            organization: contact.organizationName.isEmpty ? nil : contact.organizationName,
            job_title: contact.jobTitle.isEmpty ? nil : contact.jobTitle,
            birthday: contactBirthdayString(contact.birthday),
            notes: notes
        )
    }

    static func groups() throws -> ContactsGroupsResult {
        try requireContactsAccess()

        do {
            let groups = try contactStore.groups(matching: nil)
            let results = try groups.map { group -> ContactGroup in
                let members = try contactStore.unifiedContacts(
                    matching: CNContact.predicateForContactsInGroup(withIdentifier: group.identifier),
                    keysToFetch: [CNContactIdentifierKey as CNKeyDescriptor]
                )
                return ContactGroup(
                    id: group.identifier,
                    name: group.name,
                    count: members.count
                )
            }
            return ContactsGroupsResult(groups: results)
        } catch {
            throw RpcError(
                code: -32003,
                message: "pim.contacts.groups: \(error.localizedDescription)",
                data: nil
            )
        }
    }
}
