# Phase 4 Lesson: Don't Verify APIs from Codebase Read

**Issue:** I claimed to "verify" NSH API by reading files, but my reading was incomplete and wrong.

**Task D's Correction:** Actually tested against NSH source before coding. Found:
- APIs I listed don't exist
- APIs I claimed didn't exist do exist  
- Generic parameters I missed

**Rule:** 
- Reading codebase ≠ verification
- Actual subagent testing against real source > my file reads
- For external APIs (NSH), trust the system that uses it, not my reading

**Lesson for Task E & Rest of Phase 4:**
- Don't assume I can "verify" architecture by reading docs
- Have subagents test assumptions against live system
- Wrong assumptions cascaded into bad plans (Task B corrections, Task D errors)

**Going Forward:**
- Task D uses real NSH source + integration tests ✅
- Task B uses real codebase queries ✅  
- Task C uses real RPC introspection ✅
- Main agent integrates their findings, doesn't re-verify
