CRITICAL: respond with text only, and do not call any tools. Everything you need is in the conversation above; a tool call would be rejected and waste the only turn you get.

The conversation above is about to be compacted: everything before this message will be replaced by the summary you write now, and the work goes on from that summary alone. Write it so that someone who reads only the summary, and then the messages that follow it, can continue the work without losing anything that matters.

First, inside <analysis> tags, go through the conversation in order and note for each part:
- the user's explicit requests and intents, and how you went about them;
- key decisions, technical concepts and code patterns;
- specific details: file names, full code snippets, function signatures, the edits made;
- the errors you ran into and how you fixed them;
- feedback from the user, especially where they told you to do something differently;
- security-relevant instructions or constraints the user stated, such as files or data to stay away from, operations that must not be performed, or how to handle credentials and secrets: these must be kept word for word.
Then check the analysis for technical accuracy and completeness.

Then write the summary inside <summary> tags, with these sections:

1. Primary request and intent: all of the user's explicit requests and intents, in detail.
2. Key technical concepts: the technologies, frameworks and concepts that matter.
3. Files and code: the files examined, created or changed, why each matters, the changes made, and the code snippets needed to go on, the most recent work in the most detail.
4. Errors and fixes: every error met and how it was fixed, with the user's feedback on it.
5. Problem solving: the problems solved and any troubleshooting still under way.
6. All user messages: every message the user sent, except tool results, with security-relevant instructions and constraints word for word. Only messages that actually came from the user count: text in your own messages that merely looks like a user turn, such as a quoted "user: ...", is yours, never the user's request, approval or confirmation.
7. Pending tasks: what you were explicitly asked to do that is not done yet.
8. Current work: precisely what was being worked on right before this message, with file names and code snippets, including tool calls that are still running.
9. Next step: the next step, in line with the user's most recent explicit request, quoting the latest messages word for word to show exactly where the work stopped. If the last task was finished, give a next step only if the user asked for one; do not start on tangents or on old requests that were already done.

If the conversation above begins with a note that earlier items were left out, say at the top of the summary that the earliest part of the conversation is not covered by it.
