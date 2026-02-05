Feature: Chain-of-Thought Transparency
  As an AI system with transparent reasoning
  I want to parse LLM reasoning into structured steps
  So that users can understand the AI's thought process

  # ═══ Reasoning Step Classification ═══

  Scenario: Classify observation step
    Given reasoning text "Looking at the request, I see the user wants help"
    When I parse the structured thinking
    Then the first step should be type "Observation"

  Scenario: Classify analysis step
    Given reasoning text "Considering the options: we could use A or B"
    When I parse the structured thinking
    Then the first step should be type "Analysis"

  Scenario: Classify planning step
    Given reasoning text "I'll start by reading the file, then analyzing it"
    When I parse the structured thinking
    Then the first step should be type "Planning"

  Scenario: Classify decision step
    Given reasoning text "Therefore, I will use the search tool"
    When I parse the structured thinking
    Then the first step should be type "Decision"

  Scenario: Classify reflection step
    Given reasoning text "Wait, let me reconsider this approach"
    When I parse the structured thinking
    Then the first step should be type "Reflection"

  Scenario: Classify risk assessment step
    Given reasoning text "There's a risk that this might fail if the file is large"
    When I parse the structured thinking
    Then the first step should be type "RiskAssessment"

  # ═══ Confidence Detection ═══

  Scenario: Detect high confidence
    Given reasoning text "I'm confident this is the right approach"
    When I parse the structured thinking
    Then the confidence should be "High"

  Scenario: Detect medium confidence
    Given reasoning text "I think this should work for most cases"
    When I parse the structured thinking
    Then the confidence should be "Medium"

  Scenario: Detect low confidence
    Given reasoning text "I'm not sure, but this might be the solution"
    When I parse the structured thinking
    Then the confidence should be "Low"

  Scenario: Detect exploratory confidence
    Given reasoning text "Let's try this approach and see what happens"
    When I parse the structured thinking
    Then the confidence should be "Exploratory"

  # ═══ Alternative and Uncertainty Detection ═══

  Scenario: Extract alternatives from reasoning
    Given reasoning text "Alternatively, we could use a different library"
    When I parse the structured thinking
    Then the alternatives should contain "different library"

  Scenario: Extract uncertainties from reasoning
    Given reasoning text "I'm uncertain about the performance implications"
    When I parse the structured thinking
    Then the uncertainties should contain "performance"

  # ═══ Multi-step Reasoning ═══

  Scenario: Parse multi-step reasoning
    Given reasoning text:
      """
      Looking at the request, I see the user wants to search.
      Considering whether to use web search or file search.
      I'll use web search since the query is about external information.
      Therefore, I will call the web_search tool.
      """
    When I parse the structured thinking
    Then the structured thinking should have 4 steps
    And step 1 should be type "Observation"
    And step 2 should be type "Analysis"
    And step 3 should be type "Planning"
    And step 4 should be type "Decision"

  # ═══ DecisionParser Integration ═══

  Scenario: DecisionParser includes structured thinking
    Given a valid LLM response with reasoning "Looking at this, I will search. Therefore, I'll use web_search."
    When I parse the decision
    Then the decision should have structured thinking
    And the structured thinking should have at least 1 step

  Scenario: No structured thinking when no reasoning
    Given a valid LLM response with no reasoning
    When I parse the decision
    Then the decision should not have structured thinking

  # ═══ Thinking Guidance in Prompt ═══

  Scenario: Thinking guidance appears when enabled
    Given a prompt builder with thinking transparency enabled
    When I build the system prompt
    Then the prompt should contain "Thinking Transparency"
    And the prompt should contain "Reasoning Flow"
    And the prompt should contain "Observation"
    And the prompt should contain "Expressing Uncertainty"

  Scenario: Thinking guidance absent by default
    Given a default prompt builder
    When I build the system prompt
    Then the prompt should not contain "Thinking Transparency"
