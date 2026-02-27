import Contacts
import Foundation
import os

/// Service for interacting with macOS Contacts via Contacts.framework.
///
/// Provides CRUD operations on contacts exposed as Bridge handlers.
/// Each method accepts `[String: AnyCodable]` params and returns
/// `Result<AnyCodable, BridgeServer.HandlerError>`, matching the
/// `BridgeServer.Handler` signature.
final class ContactsService {

    // MARK: - Singleton

    static let shared = ContactsService()

    // MARK: - Properties

    private let store = CNContactStore()
    private let logger = Logger(subsystem: "com.aleph.app", category: "ContactsService")

    /// Keys to fetch for contact queries.
    private let fetchKeys: [CNKeyDescriptor] = [
        CNContactIdentifierKey as CNKeyDescriptor,
        CNContactGivenNameKey as CNKeyDescriptor,
        CNContactFamilyNameKey as CNKeyDescriptor,
        CNContactOrganizationNameKey as CNKeyDescriptor,
        CNContactPhoneNumbersKey as CNKeyDescriptor,
        CNContactEmailAddressesKey as CNKeyDescriptor,
        CNContactPostalAddressesKey as CNKeyDescriptor,
        CNContactNoteKey as CNKeyDescriptor,
    ]

    private init() {}

    // MARK: - Access

    /// Request Contacts access from the user.
    ///
    /// Uses `CNContactStore.requestAccess(for: .contacts)`.
    /// Blocks the calling thread until the user responds.
    func ensureAccess() -> Result<Void, BridgeServer.HandlerError> {
        let semaphore = DispatchSemaphore(value: 0)
        var granted = false
        var accessError: Error?

        store.requestAccess(for: .contacts) { ok, error in
            granted = ok
            accessError = error
            semaphore.signal()
        }

        semaphore.wait()

        if let error = accessError {
            logger.error("Contacts access error: \(error.localizedDescription)")
            return .failure(.init(
                code: PIMErrorCode.permissionDenied,
                message: "Contacts access denied: \(error.localizedDescription)"
            ))
        }

        guard granted else {
            return .failure(.init(
                code: PIMErrorCode.permissionDenied,
                message: "Contacts access denied. Enable in System Settings > Privacy & Security > Contacts."
            ))
        }

        return .success(())
    }

    // MARK: - Search Contacts

    /// Search contacts by name.
    ///
    /// Params:
    /// - `query` (required): Name string to search for.
    ///
    /// Returns: `{ "contacts": [{ "id", "given_name", ... }] }`
    func searchContacts(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let query = params["query"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: query (string)"
            ))
        }

        let predicate = CNContact.predicateForContacts(matchingName: query)
        let request = CNContactFetchRequest(keysToFetch: fetchKeys)
        request.predicate = predicate

        var contacts: [[String: AnyCodable]] = []
        do {
            try store.enumerateContacts(with: request) { contact, _ in
                contacts.append(contactToDict(contact))
            }
        } catch {
            logger.error("Failed to search contacts: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to search contacts: \(error.localizedDescription)"
            ))
        }

        let contactDicts: [AnyCodable] = contacts.map { AnyCodable($0) }
        return .success(AnyCodable(["contacts": AnyCodable(contactDicts)]))
    }

    // MARK: - Get Contact

    /// Get a single contact by identifier.
    ///
    /// Params:
    /// - `id` (required): Contact identifier string.
    ///
    /// Returns: `{ "contact": { "id", "given_name", ... } }`
    func getContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let contactId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        let predicate = CNContact.predicateForContacts(withIdentifiers: [contactId])

        do {
            let contacts = try store.unifiedContacts(matching: predicate, keysToFetch: fetchKeys)
            guard let contact = contacts.first else {
                return .failure(.init(
                    code: PIMErrorCode.notFound,
                    message: "Contact not found: \(contactId)"
                ))
            }
            return .success(AnyCodable(["contact": AnyCodable(contactToDict(contact))]))
        } catch {
            logger.error("Failed to get contact: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to get contact: \(error.localizedDescription)"
            ))
        }
    }

    // MARK: - Create Contact

    /// Create a new contact.
    ///
    /// Params:
    /// - `given_name` (required): First name.
    /// - `family_name` (optional): Last name.
    /// - `organization` (optional): Company/organization.
    /// - `notes` (optional): Notes text.
    /// - `phone_numbers` (optional): Array of phone number strings.
    /// - `emails` (optional): Array of email strings.
    ///
    /// Returns: `{ "contact": { ... } }`
    func createContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let givenName = params["given_name"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: given_name (string)"
            ))
        }

        let contact = CNMutableContact()
        contact.givenName = givenName

        if let familyName = params["family_name"]?.stringValue {
            contact.familyName = familyName
        }
        if let organization = params["organization"]?.stringValue {
            contact.organizationName = organization
        }
        if let notes = params["notes"]?.stringValue {
            contact.note = notes
        }

        // Phone numbers
        if let phoneArray = params["phone_numbers"]?.arrayValue {
            contact.phoneNumbers = phoneArray.compactMap { item in
                guard let number = item.stringValue else { return nil }
                return CNLabeledValue(
                    label: CNLabelPhoneNumberMain,
                    value: CNPhoneNumber(stringValue: number)
                )
            }
        }

        // Emails
        if let emailArray = params["emails"]?.arrayValue {
            contact.emailAddresses = emailArray.compactMap { item in
                guard let email = item.stringValue else { return nil }
                return CNLabeledValue(label: CNLabelHome, value: email as NSString)
            }
        }

        let saveRequest = CNSaveRequest()
        saveRequest.add(contact, toContainerWithIdentifier: nil)

        do {
            try store.execute(saveRequest)
        } catch {
            logger.error("Failed to create contact: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to create contact: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["contact": AnyCodable(contactToDict(contact))]))
    }

    // MARK: - Update Contact

    /// Update an existing contact.
    ///
    /// Params:
    /// - `id` (required): Contact identifier.
    /// - `given_name` (optional): New first name.
    /// - `family_name` (optional): New last name.
    /// - `organization` (optional): New organization.
    /// - `notes` (optional): New notes.
    /// - `phone_numbers` (optional): New array of phone number strings (replaces existing).
    /// - `emails` (optional): New array of email strings (replaces existing).
    ///
    /// Returns: `{ "contact": { ... } }`
    func updateContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let contactId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        // Fetch existing contact
        let predicate = CNContact.predicateForContacts(withIdentifiers: [contactId])
        let contacts: [CNContact]
        do {
            contacts = try store.unifiedContacts(matching: predicate, keysToFetch: fetchKeys)
        } catch {
            logger.error("Failed to fetch contact for update: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to fetch contact: \(error.localizedDescription)"
            ))
        }

        guard let existing = contacts.first else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Contact not found: \(contactId)"
            ))
        }

        guard let mutable = existing.mutableCopy() as? CNMutableContact else {
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to create mutable copy of contact"
            ))
        }

        // Apply optional updates
        if let givenName = params["given_name"]?.stringValue {
            mutable.givenName = givenName
        }
        if let familyName = params["family_name"]?.stringValue {
            mutable.familyName = familyName
        }
        if let organization = params["organization"]?.stringValue {
            mutable.organizationName = organization
        }
        if let notes = params["notes"]?.stringValue {
            mutable.note = notes
        }

        // Phone numbers (replaces existing)
        if let phoneArray = params["phone_numbers"]?.arrayValue {
            mutable.phoneNumbers = phoneArray.compactMap { item in
                guard let number = item.stringValue else { return nil }
                return CNLabeledValue(
                    label: CNLabelPhoneNumberMain,
                    value: CNPhoneNumber(stringValue: number)
                )
            }
        }

        // Emails (replaces existing)
        if let emailArray = params["emails"]?.arrayValue {
            mutable.emailAddresses = emailArray.compactMap { item in
                guard let email = item.stringValue else { return nil }
                return CNLabeledValue(label: CNLabelHome, value: email as NSString)
            }
        }

        let saveRequest = CNSaveRequest()
        saveRequest.update(mutable)

        do {
            try store.execute(saveRequest)
        } catch {
            logger.error("Failed to update contact: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to update contact: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["contact": AnyCodable(contactToDict(mutable))]))
    }

    // MARK: - Delete Contact

    /// Delete a contact by identifier.
    ///
    /// Params:
    /// - `id` (required): Contact identifier.
    ///
    /// Returns: `{ "deleted": true }`
    func deleteContact(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        guard let contactId = params["id"]?.stringValue else {
            return .failure(.init(
                code: PIMErrorCode.validationFailed,
                message: "Missing required param: id (string)"
            ))
        }

        // Fetch existing contact
        let predicate = CNContact.predicateForContacts(withIdentifiers: [contactId])
        let contacts: [CNContact]
        do {
            contacts = try store.unifiedContacts(matching: predicate, keysToFetch: fetchKeys)
        } catch {
            logger.error("Failed to fetch contact for delete: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to fetch contact: \(error.localizedDescription)"
            ))
        }

        guard let existing = contacts.first else {
            return .failure(.init(
                code: PIMErrorCode.notFound,
                message: "Contact not found: \(contactId)"
            ))
        }

        guard let mutable = existing.mutableCopy() as? CNMutableContact else {
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to create mutable copy of contact"
            ))
        }

        let saveRequest = CNSaveRequest()
        saveRequest.delete(mutable)

        do {
            try store.execute(saveRequest)
        } catch {
            logger.error("Failed to delete contact: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to delete contact: \(error.localizedDescription)"
            ))
        }

        return .success(AnyCodable(["deleted": AnyCodable(true)]))
    }

    // MARK: - List Groups

    /// List all contact groups.
    ///
    /// No params required.
    ///
    /// Returns: `{ "groups": [{ "id", "name" }] }`
    func listGroups(params: [String: AnyCodable]) -> Result<AnyCodable, BridgeServer.HandlerError> {
        switch ensureAccess() {
        case .failure(let err): return .failure(err)
        case .success: break
        }

        let groups: [CNGroup]
        do {
            groups = try store.groups(matching: nil)
        } catch {
            logger.error("Failed to list groups: \(error.localizedDescription)")
            return .failure(.init(
                code: BridgeErrorCode.internal,
                message: "Failed to list groups: \(error.localizedDescription)"
            ))
        }

        let groupDicts: [AnyCodable] = groups.map { group in
            AnyCodable([
                "id": AnyCodable(group.identifier),
                "name": AnyCodable(group.name),
            ])
        }

        return .success(AnyCodable(["groups": AnyCodable(groupDicts)]))
    }

    // MARK: - Helpers

    /// Convert a CNContact to a dictionary suitable for JSON-RPC response.
    private func contactToDict(_ contact: CNContact) -> [String: AnyCodable] {
        // Phone numbers as array of strings
        let phones: [AnyCodable] = contact.phoneNumbers.map { labeled in
            AnyCodable(labeled.value.stringValue)
        }

        // Email addresses as array of strings
        let emails: [AnyCodable] = contact.emailAddresses.map { labeled in
            AnyCodable(labeled.value as String)
        }

        var dict: [String: AnyCodable] = [
            "id": AnyCodable(contact.identifier),
            "given_name": AnyCodable(contact.givenName),
            "family_name": AnyCodable(contact.familyName),
            "organization": AnyCodable(contact.organizationName),
            "phone_numbers": AnyCodable(phones),
            "emails": AnyCodable(emails),
        ]

        // Notes (may not be available if key was not fetched)
        if contact.isKeyAvailable(CNContactNoteKey) {
            let note = contact.note
            if !note.isEmpty {
                dict["notes"] = AnyCodable(note)
            } else {
                dict["notes"] = AnyCodable(NSNull())
            }
        } else {
            dict["notes"] = AnyCodable(NSNull())
        }

        return dict
    }
}
