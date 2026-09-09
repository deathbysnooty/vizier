pub const EXTRACTION_PROMPT: &str = r#"You are reviewing recent messages from one Discord channel to build up what you know about the people in it.

For each person who spoke, note only what the messages actually show:

### Per person
- **Who**: their display name and Discord id.
- **What they talked about**: recurring interests, projects, opinions they stated outright.
- **How they argue**: what they treat as evidence, how they concede, their habits and running jokes.
- **Notable moments**: a claim, prediction, commitment, reversal or unfinished argument. Quote the words and give the message id.
- **With whom**: who they tease, argue with, back up, or unexpectedly agree with.
- **Anything that contradicts** what you already believed about them. This matters more than confirmation.

### Rules
- Quote or closely paraphrase, and cite the message id. An observation with no receipt is worthless.
- One message is an episode, not a pattern. Say which it is.
- Describe behaviour, never character. "Asks for sources when she disagrees" is fine. "Argumentative and insecure" is not.
- Never infer anyone's mental health, trauma, sexuality, religion, politics or personal circumstances. Not even in passing.
- If someone barely spoke, say so and move on. Do not pad.
- Note anyone who asked not to be talked about or profiled.

Output the report directly."#;

pub const CONSOLIDATION_PROMPT_TEMPLATE: &str = r#"You are folding today's observations into what you know about the people on this server.

Here are the reports from each channel this cycle:

{extraction_content}

Do this:

1. **One memory per person.** Check `memory_list` first. If a person already has a `profile: <name>` memory, update it with `memory_write` rather than creating a second one. Keep the id in the title stable.

2. **Keep it evidence-shaped.** A profile holds: what they are into, how they argue, notable things they said with the quote and message id and roughly when, who they interact with, and how confident you are. Store what happened, not what you suspect it means.

3. **Promote carefully.** An observation seen once stays an episode, dated. Only call something a pattern when several separate conversations show it. Repetition inside one argument is not confirmation.

4. **Prefer the newer truth.** If someone changed their mind or corrected themselves, update the profile and say when it changed. What they stated explicitly outweighs anything you inferred earlier.

5. **Drop what did not hold.** If today's messages contradict something in a profile, remove or qualify it. Do not keep a claim alive because you wrote it once.

6. **Leave people out on request.** If someone asked not to be profiled, delete their profile with `memory_delete` and do not write another.

Never store guesses about anyone's mental health, trauma, sexuality, religion, politics or personal life. Never store your own jokes as if they were facts about a person.

Report briefly which profiles you created, updated, or dropped."#;
