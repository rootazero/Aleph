Feature: Memory Facts Vector Database
  As a memory system
  I want to store and search facts using vector similarity
  So that relevant context can be retrieved for AI responses

  Background:
    Given a temporary vector database

  # ═══ Vector Database Sync Tests ═══

  Scenario: Insert fact syncs to vector table for search
    Given a fact with id "fact-1" and content "Test fact" and type "preference"
    When I insert the fact into the database
    Then I should be able to search and find the fact

  Scenario: Search facts uses vec0 for similarity
    Given 3 facts with incremental embeddings
    When I search with a zero embedding and limit 2
    Then I should receive 2 results
    And all results should have similarity scores

  # ═══ FTS Query Preparation Tests ═══

  Scenario: FTS query basic tokenization
    When I prepare FTS query for "rust programming"
    Then the FTS query should match basic tokenization

  Scenario: FTS query filters stop words
    When I prepare FTS query for "the user is learning rust"
    Then the FTS query should filter stop words

  Scenario: FTS query filters single characters
    When I prepare FTS query for "I am a rust developer"
    Then the FTS query should filter single chars

  Scenario: FTS query handles empty input
    When I prepare FTS query for ""
    Then the FTS query should be empty

  Scenario: FTS query with only stop words returns empty
    When I prepare FTS query for "the a an is are"
    Then the FTS query should be empty

  Scenario: FTS query removes quotes from input
    When I prepare FTS query for input with quotes
    Then the FTS query should have escaped quotes

  # ═══ Hybrid Search Tests ═══

  Scenario: Hybrid search falls back to vector-only when text is empty
    Given a fact with id "fact-1" and content "The user prefers Rust for systems programming" and type "preference"
    And the fact has embedding value 0.5
    When I insert the fact into the database
    And I hybrid search with the same embedding and empty text
    Then I should receive 1 result
    And the first result should have a high similarity score

  Scenario: Hybrid search with text match boosts relevant results
    Given these facts exist:
      | id     | content                                       | embedding_first |
      | fact-0 | The user prefers Rust for systems programming | 0.1             |
      | fact-1 | The user likes TypeScript for web development | 0.2             |
      | fact-2 | The user is learning Python for data science  | 0.3             |
    When I hybrid search for "Rust programming" with embedding value 0.1
    Then I should receive results
    And all results should have similarity scores

  Scenario: Hybrid search respects minimum score threshold
    Given a fact with id "fact-1" and content "Test fact" and type "other"
    And the fact has embedding value 0.5
    When I insert the fact into the database
    And I hybrid search with opposite embedding and min_score 0.99
    Then I should receive 0 results

  Scenario: Hybrid search respects result limit
    Given 10 facts with sequential content
    When I hybrid search with empty text and limit 3
    Then I should receive at most 3 results

  Scenario: Hybrid search excludes invalid facts
    Given a valid fact with id "valid-fact" and content "Valid fact"
    And an invalid fact with id "invalid-fact" and content "Invalid fact"
    When I insert all facts into the database
    And I hybrid search with the shared embedding
    Then I should receive 1 result
    And the result should have id "valid-fact"
