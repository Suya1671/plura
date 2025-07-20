# Plura
A slack bot designed to make the lives of plural systems easier. Inspired by PluralKit.

<p align="center">
  <video src="assets/demo.mp4"></video>
</p>

I'm not plural myself, but I do know a lot of Hack Club members that are plural and do want an easier way to differentiate between alters. Thus, I made this.
It works similarly to PluralKit, by basically rewriting sent messages under different members using Slack's API.

## Features
- Manage members and profiles
  - Add, delete, edit, and get member information
  - Manage member aliases so your members are easier to refer to.
- Send messages under different members
  - Triggers
    - E.g. `Hi ~J` to send a message under a user who is associated with the suffix `~J`
- Message actions for managing messages sent by members
  - Message editing
  - Message deletion
  - Message info (i.e. the profile of the member that sent it)
  - Message reproxying (i.e. sending a message under a different user after it's been sent)
- Set and view information about a member

## AI Usage in this project
(_Required for Summer Of Making by Hack Club_)

- General autocompletion tooling (i.e. Zed edit predictions)
- Initial draft of the merge_trigger_fields migration was done by AI
  - This also included editing matching structs
  - This has been refactored and edited by Suya1671
- Initial draft of trigger commands was done by AI
  - Note that this has been completely redone and nearly no AI-generated code remains. Suya1671 remade triggers to have an entirely command based interface.
- General code analysis was done by AI to make sure it's easier to understand and maintain the project
  - No code edits by an agent was done. Only suggestions which were implemented if seen fit by Suya1671
