ISSUE|orchestrator.rs:116|low|Lock poisoning silently recovered when locking sessions map; corrupted state may be used|self.sessions.lock().unwrap_or_else(|e| e.into_inner()) ignores poison errors in create_session
ISSUE|orchestrator.rs:153|low|Lock poisoning silently recovered when looking up sessions; corrupted state may be used|self.sessions.lock().unwrap_or_else(|e| e.into_inner()) ignores poison errors in get_session
ISSUE|orchestrator.rs:167|low|Lock poisoning silently recovered when removing sessions; corrupted state may be used|self.sessions.lock().unwrap_or_else(|e| e.into_inner()) ignores poison errors in end_session
ISSUE|orchestrator.rs:239|low|Lock poisoning silently recovered when listing sessions; corrupted state may be used|self.sessions.lock().unwrap_or_else(|e| e.into_inner()) ignores poison errors in all_sessions
ISSUE|orchestrator.rs:170|low|Session may remain Active if try_lock fails during end_session|if let Ok(mut session) = handle.try_lock() silently skips ending when lock is contended
ISSUE|channel.rs:72|medium|Command parser accepts /groupchatstart (missing space) as a valid start command|after.starts_with("start ") matches without requiring space after /groupchat
ISSUE|channel.rs:75|medium|Command parser accepts /groupchatend (missing space) as a valid end command|after == "end" || after.starts_with("end ") matches without requiring space after /groupchat
ISSUE|channel.rs:114|low|--role flag without a value is silently ignored instead of failing|if i < tokens.len() allows missing role spec to be skipped
ISSUE|channel.rs:162|low|Inline role IDs can collide for names differing only by spaces vs hyphens|"Dr Smith" and "Dr-Smith" both map to id "dr_smith"
ISSUE|coordinator.rs:46|medium|Unescaped persona fields injected into coordinator prompt enable prompt injection|format!("- id=\"{}\" name=\"{}\" prompt=\"{}\"", p.id, p.name, truncated) interpolates user-controlled persona data
ISSUE|session.rs:96|medium|Conversation history interpolates unescaped speaker names and content into later prompts|format!("[{}]: {}\n\n", turn.speaker.name(), turn.content) embeds arbitrary turn content
ISSUE|persona.rs:75|low|reload silently overwrites duplicate persona IDs without warning|from_configs logs duplicate warnings but reload does not
ISSUE|protocol.rs:217|low|FromStr error type is bare String instead of a typed error|type Err = String lacks a structured error variant
