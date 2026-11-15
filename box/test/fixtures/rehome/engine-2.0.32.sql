PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE `account_state` (
          `id` integer PRIMARY KEY,
          `active_account_id` text,
          `active_org_id` text,
          CONSTRAINT `fk_account_state_active_account_id_account_id_fk` FOREIGN KEY (`active_account_id`) REFERENCES `account`(`id`) ON DELETE SET NULL
        );
CREATE TABLE `account` (
          `id` text PRIMARY KEY,
          `email` text NOT NULL,
          `url` text NOT NULL,
          `access_token` text NOT NULL,
          `refresh_token` text NOT NULL,
          `token_expiry` integer,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `control_account` (
          `email` text NOT NULL,
          `url` text NOT NULL,
          `access_token` text NOT NULL,
          `refresh_token` text NOT NULL,
          `token_expiry` integer,
          `active` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `control_account_pk` PRIMARY KEY(`email`, `url`)
        );
CREATE TABLE `credential` (
          `id` text PRIMARY KEY,
          `integration_id` text,
          `label` text NOT NULL,
          `value` text NOT NULL,
          `connector_id` text,
          `method_id` text,
          `active` integer,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `event_sequence` (
          `aggregate_id` text PRIMARY KEY,
          `seq` integer NOT NULL,
          `owner_id` text
        );
INSERT INTO event_sequence VALUES('@ROOT@',0,NULL);
INSERT INTO event_sequence VALUES('ses_e1f290557ffeEzeWbOYhplyf1n',10,NULL);
INSERT INTO event_sequence VALUES('ses_e1f2902d8ffecmgtpdgKfSv9dT',15,NULL);
INSERT INTO event_sequence VALUES('ses_e1f290231ffem8O3cbwyLckp1C',0,NULL);
INSERT INTO event_sequence VALUES('ses_e1f290212ffejK1oLIGU04GqkK',10,NULL);
INSERT INTO event_sequence VALUES('ses_e1f290128ffeFZrAVqV9A0yaZp',10,NULL);
CREATE TABLE `event` (
          `id` text PRIMARY KEY,
          `aggregate_id` text NOT NULL,
          `seq` integer NOT NULL,
          `created` integer DEFAULT 0 NOT NULL,
          `type` text NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_event_aggregate_id_event_sequence_aggregate_id_fk` FOREIGN KEY (`aggregate_id`) REFERENCES `event_sequence`(`aggregate_id`) ON DELETE CASCADE
        );
CREATE TABLE `kv` (
          `key` text PRIMARY KEY,
          `value` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
CREATE TABLE `permission` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `action` text NOT NULL,
          `resource` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `fk_permission_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `project_directory` (
          `project_id` text NOT NULL,
          `directory` text NOT NULL,
          `type` text,
          `strategy` text,
          `time_created` integer NOT NULL,
          CONSTRAINT `project_directory_pk` PRIMARY KEY(`project_id`, `directory`),
          CONSTRAINT `fk_project_directory_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `project` (
          `id` text PRIMARY KEY,
          `worktree` text NOT NULL,
          `vcs` text,
          `name` text,
          `icon_url` text,
          `icon_url_override` text,
          `icon_color` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_initialized` integer,
          `time_active` integer DEFAULT 0 NOT NULL,
          `time_archived` integer,
          `sandboxes` text NOT NULL,
          `commands` text
        );
INSERT INTO project VALUES('@ROOT@','@HOME@/proj','git',NULL,NULL,NULL,NULL,1794773547757,1794773547757,NULL,1794773547785,NULL,'[]',NULL);
INSERT INTO project VALUES('@ID:@S@/Chats/c1@','@S@/Chats/c1',NULL,NULL,NULL,NULL,NULL,1794773548328,1794773548328,NULL,1794773548330,NULL,'[]',NULL);
INSERT INTO project VALUES('@ID:@HOME@/Projects/plain@','@HOME@/Projects/plain',NULL,NULL,NULL,NULL,NULL,1794773548760,1794773548760,NULL,1794773548762,NULL,'[]',NULL);
CREATE TABLE `instruction_blob` (
          `hash` text PRIMARY KEY,
          `value` text
        );
INSERT INTO instruction_blob VALUES('e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b','{"total":8,"shown":8,"namespaces":[{"name":"opencode","description":"Tools for managing OpenCode itself, such as working with sessions and worktrees, searching the available models, and reading MCP resources.","count":8,"entries":[{"path":"opencode.list_mcp_resources","line":"  - tools.opencode.list_mcp_resources({\n  /** MCP server name as configured. Omit to list every server. */\n  server?: string,\n}): Promise<{\n  resources: Array<{\n      server: string,\n      name: string,\n      uri: string,\n      description?: string,\n      mimeType?: string,\n    }>,\n  templates: Array<{\n      server: string,\n      name: string,\n      uriTemplate: string,\n      description?: string,\n      mimeType?: string,\n    }>,\n}> // List documents, records, and other data exposed by MCP servers. Use this when the user refers to something that is no..."},{"path":"opencode.models","line":"  - tools.opencode.models({\n  /** Text to search for in model names and IDs. */\n  query?: string,\n  /** Provider ID or name to filter by. Try your own provider first. */\n  provider?: string,\n  /** Include older versions of each model family. By default only the newest version is listed. */\n  all?: boolean,\n  /** Maximum number of models to return. Defaults to 20. @integer @minimum 1 @maximum 100 */\n  limit?: number,\n  /** Number of models to skip, for paging through results. @integer @minimum 0 */\n  offset?: number,\n}): Promise<{\n  /** Matching models grouped by provider. Your own provider comes first. */\n  providers: Array<{\n      id: string,\n      name: string,\n      /** Newest first. */\n      models: Array<{\n          /** providerID/modelID */\n          id: string,\n          name: string,\n          /** Release date as a Unix timestamp in milliseconds, or 0 when unknown. */\n          released: number,\n          variants: Array<string>,\n          /** Pricing in USD per million tokens. */\n          cost: Array<{\n              tier?: {\n                type: \"context\",\n                /** @integer */\n                size: number,\n              },\n              input: number,\n              output: number,\n              cache: {\n                read: number,\n                write: number,\n              },\n            }>,\n          status: \"alpha\" | \"beta\" | \"deprecated\" | \"active\",\n        }>,\n    }>,\n  /** Number of matching models across all pages. @integer */\n  total: number,\n  /** Offset of the next page, or null on the last page. */\n  next: number | null,\n}> // Search the models available to use. Use this to turn a model name the user mentions into an exact reference before ru..."},{"path":"opencode.read_mcp_resource","line":"  - tools.opencode.read_mcp_resource({\n  /** The server field of the discovered resource. */\n  server: string,\n  /** Exact resource URI from discovery, an expanded uriTemplate, or the user. */\n  uri: string,\n}): Promise<{\n  server: string,\n  uri: string,\n  contents: Array<{\n        type: \"text\",\n        uri: string,\n        text: string,\n        mimeType?: string,\n      } | {\n        type: \"blob\",\n        uri: string,\n        blob: string,\n        mimeType?: string,\n      }>,\n}> // Read one MCP resource by server and URI. Not for local files. Always return the full contents rather than slicing or ..."},{"path":"opencode.session_move","line":"  - tools.opencode.session_move({\n  /** Omit to move the current session. @pattern ^ses */\n  sessionID?: string,\n  /** Destination directory, relative to the target session''s directory or absolute. Supports ~. @minLength 1 */\n  directory: string,\n}): Promise<{\n  /** @pattern ^ses */\n  sessionID: string,\n  directory: string,\n}> // Move a session to another directory, or omit sessionID to move the current session. The current session moves at the ..."},{"path":"opencode.session_rename","line":"  - tools.opencode.session_rename({\n  /** Omit to rename the current session. @pattern ^ses */\n  sessionID?: string,\n  /** New session title. @minLength 1 */\n  title: string,\n}): Promise<{\n  /** @pattern ^ses */\n  sessionID: string,\n  title: string,\n}> // Rename a session, or omit sessionID to rename the current session. Use a short, specific title that summarizes the wo..."},{"path":"opencode.worktree_create","line":"  - tools.opencode.worktree_create({\n  name: string,\n}): Promise<{\n  directory: string,\n}> // Create a named worktree. Returns its directory."},{"path":"opencode.worktree_list","line":"  - tools.opencode.worktree_list(): Promise<{\n  worktrees: Array<{\n      directory: string,\n      strategy?: string,\n    }>,\n}> // List a repository''s worktrees."},{"path":"opencode.worktree_remove","line":"  - tools.opencode.worktree_remove({\n  /** Absolute path of the worktree to remove. @minLength 1 */\n  directory: string,\n  /** Discard modified and untracked files. Only set this when the user has confirmed forced removal. */\n  force?: boolean,\n}): Promise<{\n  directory: string,\n  removed: boolean,\n}> // Remove a worktree. The repository location selects configuration; directory identifies the worktree to remove."}]}]}');
INSERT INTO instruction_blob VALUES('8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9','[{"id":"opencode","name":"OpenCode","description":"Use this skill for any question about OpenCode itself, including how OpenCode works, using or configuring it, migrating from V1 to V2, troubleshooting it, developing plugins or integrations, using the OpenCode SDK, clients, server, or API, and contributing to the OpenCode codebase. Also use it for OpenCode agents, commands, skills, tools, permissions, MCP servers, providers, models, themes, keybinds, formatters, the CLI, TUI, desktop app, and web app."},{"id":"report","name":"Report","description":"Use when the user wants to report an opencode issue or bug. Collect standard diagnostics, add user-specific reproduction context, and publish the issue with GitHub CLI."}]');
INSERT INTO instruction_blob VALUES('2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1','"Sun Nov 15 2026"');
INSERT INTO instruction_blob VALUES('e20bfc56f17094582cc939849491bafb6df05bba2d4f1c75f8037c967b67734e','"<env>\n  Working directory: @HOME@/proj\n  Workspace root folder: @HOME@/proj\n  Is directory a git repo: yes\n  Platform: darwin\n  Prefer /tmp/kuvo-0.7/opencode over generic system temporary directories such as /tmp; it is pre-created and approved for external access.\n</env>"');
INSERT INTO instruction_blob VALUES('54dd19f5acd10c90f20645182ba104a75f79216e7138bc1bb7c31736f02bf3b4','"<env>\n  Working directory: @S@/Chats/c1\n  Workspace root folder: @S@/Chats/c1\n  Is directory a git repo: no\n  Platform: darwin\n  Prefer /tmp/kuvo-0.7/opencode over generic system temporary directories such as /tmp; it is pre-created and approved for external access.\n</env>"');
INSERT INTO instruction_blob VALUES('47a894cf8f422539046a31c259101eb7ddc45879e8c61c5e11860e50ff866d02','"<env>\n  Working directory: @S@/opencode/data/opencode/worktree/@ROOT6@/feature one\n  Workspace root folder: @S@/opencode/data/opencode/worktree/@ROOT6@/feature one\n  Is directory a git repo: yes\n  Platform: darwin\n  Prefer /tmp/kuvo-0.7/opencode over generic system temporary directories such as /tmp; it is pre-created and approved for external access.\n</env>"');
INSERT INTO instruction_blob VALUES('0d04c416ad54339ba8bb4da5e128a5fd79bce81c2e02c52e17702c1c9c4b0111','"<env>\n  Working directory: @HOME@/Projects/plain\n  Workspace root folder: @HOME@/Projects/plain\n  Is directory a git repo: no\n  Platform: darwin\n  Prefer /tmp/kuvo-0.7/opencode over generic system temporary directories such as /tmp; it is pre-created and approved for external access.\n</env>"');
CREATE TABLE `instruction_entry` (
          `session_id` text NOT NULL,
          `key` text NOT NULL,
          `value` text,
          `removed` integer DEFAULT false NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          CONSTRAINT `instruction_entry_pk` PRIMARY KEY(`session_id`, `key`),
          CONSTRAINT `fk_instruction_entry_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `instruction_state` (
          `session_id` text PRIMARY KEY,
          `epoch_start` integer NOT NULL,
          `through_seq` integer NOT NULL,
          `initial_values` text NOT NULL,
          `current_values` text NOT NULL,
          CONSTRAINT `fk_instruction_state_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
INSERT INTO instruction_state VALUES('ses_e1f290557ffeEzeWbOYhplyf1n',3,3,'{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"e20bfc56f17094582cc939849491bafb6df05bba2d4f1c75f8037c967b67734e"}','{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"e20bfc56f17094582cc939849491bafb6df05bba2d4f1c75f8037c967b67734e"}');
INSERT INTO instruction_state VALUES('ses_e1f2902d8ffecmgtpdgKfSv9dT',3,3,'{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"54dd19f5acd10c90f20645182ba104a75f79216e7138bc1bb7c31736f02bf3b4"}','{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"54dd19f5acd10c90f20645182ba104a75f79216e7138bc1bb7c31736f02bf3b4"}');
INSERT INTO instruction_state VALUES('ses_e1f290231ffem8O3cbwyLckp1C',0,0,'{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"54dd19f5acd10c90f20645182ba104a75f79216e7138bc1bb7c31736f02bf3b4"}','{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"54dd19f5acd10c90f20645182ba104a75f79216e7138bc1bb7c31736f02bf3b4"}');
INSERT INTO instruction_state VALUES('ses_e1f290212ffejK1oLIGU04GqkK',3,3,'{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"47a894cf8f422539046a31c259101eb7ddc45879e8c61c5e11860e50ff866d02"}','{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"47a894cf8f422539046a31c259101eb7ddc45879e8c61c5e11860e50ff866d02"}');
INSERT INTO instruction_state VALUES('ses_e1f290128ffeFZrAVqV9A0yaZp',3,3,'{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"0d04c416ad54339ba8bb4da5e128a5fd79bce81c2e02c52e17702c1c9c4b0111"}','{"core/codemode":"e6428fd7742161f6c3a6c86e53cbbe2c6e4dcc462bbf1a1a07bc4fcbeb9e0b0b","core/skill-guidance":"8394362d28c82fa4d13c7a9f5568c05680f6687e07db8f426a0e3c92f3027dc9","core/date":"2b193b1507a9510b1d26e3cfbee5a9fffd8aad6c9e6f1c4387a6c344344e45b1","core/environment":"0d04c416ad54339ba8bb4da5e128a5fd79bce81c2e02c52e17702c1c9c4b0111"}');
CREATE TABLE `session_inbox` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `payload` text NOT NULL,
          `delivery` text NOT NULL,
          `enqueued_seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          CONSTRAINT `fk_session_inbox_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
INSERT INTO session_inbox VALUES('msg_1e0d6fdca001G2uIOsernRhZm6','ses_e1f2902d8ffecmgtpdgKfSv9dT','synthetic','{"text":"The following shell command was executed by the user:\n<shell id=\"sh_1e0d6fdc40013b73WLYKiAVcD7\" state=\"completed\" command=\"pwd\">\n@S@/Chats/c1\n\n</shell>","metadata":{"source":"shell","shellID":"sh_1e0d6fdc40013b73WLYKiAVcD7","state":"completed","truncated":false,"exit":0}}','steer',13,1794773548490);
CREATE TABLE `session_message` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL,
          CONSTRAINT `fk_session_message_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
INSERT INTO session_message VALUES('msg_00000000000000000000000001','ses_e1f290557ffeEzeWbOYhplyf1n','user',4,1794773548214,1794773548214,'{"time":{"created":1794773548214},"text":"In a.txt, replace line two."}');
INSERT INTO session_message VALUES('msg_1e0d6fcb800124vMioCxyl2qYc','ses_e1f290557ffeEzeWbOYhplyf1n','assistant',5,1794773548219,1794773548269,'{"time":{"created":1794773548219,"streamed":1794773548260,"completed":1794773548269},"agent":"build","model":{"id":"m","providerID":"lab"},"content":[{"type":"text","text":"Done, as you asked."}],"snapshot":{"start":"da2310612e3cbe9ea45011418b30d130a4814633","end":"da2310612e3cbe9ea45011418b30d130a4814633","files":[]},"finish":"stop","rawFinish":"stop","cost":0,"tokens":{"input":5,"output":5,"reasoning":0,"cache":{"read":0,"write":0}}}');
INSERT INTO session_message VALUES('msg_1e0d6fcee002ZqAn7uyRqJ4zwQ','ses_e1f290557ffeEzeWbOYhplyf1n','idle',10,1794773548270,1794773548270,'{"time":{"created":1794773548270},"outcome":"succeeded"}');
INSERT INTO session_message VALUES('msg_00000000000000000000000002','ses_e1f2902d8ffecmgtpdgKfSv9dT','user',4,1794773548370,1794773548370,'{"time":{"created":1794773548370},"text":"see attached","files":[{"data":"UE5HREFUQQ==","mime":"text/plain","source":{"type":"uri","uri":"file://@S_URL@/Attachments/ses_a/b.png"},"name":"b.png"},{"data":"UE5HREFUQQ==","mime":"text/plain","source":{"type":"uri","uri":"file://@S_URL@/Attachments/ses_a/with%20space.png"},"name":"with space.png"}]}');
INSERT INTO session_message VALUES('msg_1e0d6fd53001iqMR5uoHw1q9oR','ses_e1f2902d8ffecmgtpdgKfSv9dT','assistant',5,1794773548373,1794773548377,'{"time":{"created":1794773548373,"streamed":1794773548376,"completed":1794773548377},"agent":"build","model":{"id":"m","providerID":"lab"},"content":[{"type":"text","text":"Done, as you asked."}],"finish":"stop","rawFinish":"stop","cost":0,"tokens":{"input":5,"output":5,"reasoning":0,"cache":{"read":0,"write":0}}}');
INSERT INTO session_message VALUES('msg_1e0d6fd59003ovfz09oZTKJff3','ses_e1f2902d8ffecmgtpdgKfSv9dT','idle',10,1794773548377,1794773548377,'{"time":{"created":1794773548377},"outcome":"succeeded"}');
INSERT INTO session_message VALUES('msg_1e0d6fdc5002zZUzLTcAdr6lUn','ses_e1f2902d8ffecmgtpdgKfSv9dT','shell',11,1794773548485,1794773548489,'{"metadata":{"background":true},"time":{"created":1794773548485,"completed":1794773548489},"shellID":"sh_1e0d6fdc40013b73WLYKiAVcD7","command":"pwd","status":"exited","exit":0,"output":{"output":"@S@/Chats/c1\n","cursor":77,"size":77,"truncated":false}}');
INSERT INTO session_message VALUES('msg_00000000000000000000000003','ses_e1f290212ffejK1oLIGU04GqkK','user',4,1794773548647,1794773548647,'{"time":{"created":1794773548647},"text":"Append a line to a.txt."}');
INSERT INTO session_message VALUES('msg_1e0d6fe68001KAdSXONW6qoB94','ses_e1f290212ffejK1oLIGU04GqkK','assistant',5,1794773548650,1794773548696,'{"time":{"created":1794773548650,"streamed":1794773548688,"completed":1794773548696},"agent":"build","model":{"id":"m","providerID":"lab"},"content":[{"type":"text","text":"Done, as you asked."}],"snapshot":{"start":"da2310612e3cbe9ea45011418b30d130a4814633","end":"da2310612e3cbe9ea45011418b30d130a4814633","files":[]},"finish":"stop","rawFinish":"stop","cost":0,"tokens":{"input":5,"output":5,"reasoning":0,"cache":{"read":0,"write":0}}}');
INSERT INTO session_message VALUES('msg_1e0d6fe990026qxde2WzA0fjQJ','ses_e1f290212ffejK1oLIGU04GqkK','idle',10,1794773548697,1794773548697,'{"time":{"created":1794773548697},"outcome":"succeeded"}');
INSERT INTO session_message VALUES('msg_00000000000000000000000004','ses_e1f290128ffeFZrAVqV9A0yaZp','user',4,1794773548802,1794773548802,'{"time":{"created":1794773548802},"text":"hello"}');
INSERT INTO session_message VALUES('msg_1e0d6ff03001aNafvrccPUhe8f','ses_e1f290128ffeFZrAVqV9A0yaZp','assistant',5,1794773548805,1794773548809,'{"time":{"created":1794773548805,"streamed":1794773548809,"completed":1794773548809},"agent":"build","model":{"id":"m","providerID":"lab"},"content":[{"type":"text","text":"Done, as you asked."}],"finish":"stop","rawFinish":"stop","cost":0,"tokens":{"input":5,"output":5,"reasoning":0,"cache":{"read":0,"write":0}}}');
INSERT INTO session_message VALUES('msg_1e0d6ff09004yZfTjTUO4v2xop','ses_e1f290128ffeFZrAVqV9A0yaZp','idle',10,1794773548809,1794773548809,'{"time":{"created":1794773548809},"outcome":"succeeded"}');
CREATE TABLE `session_pending` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `data` text NOT NULL,
          `delivery` text,
          `admitted_seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          CONSTRAINT `fk_session_pending_session_id_session_v2_id_fk` FOREIGN KEY (`session_id`) REFERENCES `session_v2`(`id`) ON DELETE CASCADE
        );
CREATE TABLE `session_v2` (
          `id` text PRIMARY KEY,
          `project_id` text NOT NULL,
          `workspace_id` text,
          `parent_id` text,
          `fork_session_id` text,
          `fork_boundary` text,
          `slug` text NOT NULL,
          `directory` text NOT NULL,
          `path` text,
          `title` text,
          `version` text NOT NULL,
          `share_url` text,
          `summary_additions` integer,
          `summary_deletions` integer,
          `summary_files` integer,
          `summary_diffs` text,
          `metadata` text,
          `cost` real DEFAULT 0 NOT NULL,
          `tokens_input` integer DEFAULT 0 NOT NULL,
          `tokens_output` integer DEFAULT 0 NOT NULL,
          `tokens_reasoning` integer DEFAULT 0 NOT NULL,
          `tokens_cache_read` integer DEFAULT 0 NOT NULL,
          `tokens_cache_write` integer DEFAULT 0 NOT NULL,
          `revert` text,
          `permission` text,
          `agent` text,
          `model` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `time_idle` integer,
          `time_viewed` integer,
          `idle_outcome` text,
          `time_compacting` integer,
          `time_archived` integer,
          `time_suspended` integer,
          `resume_attempts` integer DEFAULT 0 NOT NULL,
          CONSTRAINT `fk_session_v2_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
INSERT INTO session_v2 VALUES('ses_e1f290557ffeEzeWbOYhplyf1n','@ROOT@',NULL,NULL,NULL,NULL,'quiet-forest','@HOME@/proj','','git edit','2.0.32',NULL,NULL,NULL,NULL,NULL,NULL,0.0,5,5,0,0,0,NULL,NULL,NULL,NULL,1794773547759,1794773548269,1794773548270,NULL,'succeeded',NULL,NULL,NULL,0);
INSERT INTO session_v2 VALUES('ses_e1f2902d8ffecmgtpdgKfSv9dT','@ID:@S@/Chats/c1@',NULL,NULL,NULL,NULL,'quiet-eagle','@S@/Chats/c1','','chat','2.0.32',NULL,NULL,NULL,NULL,NULL,'{"kuvo":{"folder":"@S@/Chats/c1"}}',0.0,5,5,0,0,0,NULL,'[{"action":"edit","resource":"@S@/Chats/c1/*","effect":"allow"}]',NULL,NULL,1794773548328,1794773548492,1794773548377,NULL,'succeeded',NULL,NULL,NULL,0);
INSERT INTO session_v2 VALUES('ses_e1f290231ffem8O3cbwyLckp1C','@ID:@S@/Chats/c1@',NULL,NULL,'ses_e1f2902d8ffecmgtpdgKfSv9dT','{"type":"before","messageID":"msg_00000000000000000000000002"}','witty-harbor','@S@/Chats/c1','','chat (fork #1)','2.0.32',NULL,NULL,NULL,NULL,NULL,'{"kuvo":{"folder":"@S@/Chats/c1"}}',0.0,0,0,0,0,0,NULL,'[{"action":"edit","resource":"@S@/Chats/c1/*","effect":"allow"}]',NULL,NULL,1794773548495,1794773548495,NULL,NULL,NULL,NULL,NULL,NULL,0);
INSERT INTO session_v2 VALUES('ses_e1f290212ffejK1oLIGU04GqkK','@ROOT@',NULL,NULL,NULL,NULL,'glowing-garden','@S@/opencode/data/opencode/worktree/@ROOT6@/feature one','','in worktree','2.0.32',NULL,NULL,NULL,NULL,NULL,NULL,0.0,5,5,0,0,0,NULL,NULL,NULL,NULL,1794773548550,1794773548696,1794773548697,NULL,'succeeded',NULL,NULL,NULL,0);
INSERT INTO session_v2 VALUES('ses_e1f290128ffeFZrAVqV9A0yaZp','@ID:@HOME@/Projects/plain@',NULL,NULL,NULL,NULL,'tidy-island','@HOME@/Projects/plain','','plain','2.0.32',NULL,NULL,NULL,NULL,NULL,NULL,0.0,5,5,0,0,0,NULL,NULL,NULL,NULL,1794773548760,1794773548809,1794773548809,NULL,'succeeded',NULL,NULL,NULL,0);
CREATE TABLE `workspace` (
          `id` text PRIMARY KEY,
          `provider` text NOT NULL,
          `binding` text,
          `created_at` integer NOT NULL,
          `last_used_at` integer NOT NULL
        );
CREATE TABLE `worktree` (
          `project_id` text NOT NULL,
          `directory` text NOT NULL,
          `strategy` text,
          `time_created` integer NOT NULL,
          CONSTRAINT `worktree_pk` PRIMARY KEY(`project_id`, `directory`),
          CONSTRAINT `fk_worktree_project_id_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `project`(`id`) ON DELETE CASCADE
        );
INSERT INTO worktree VALUES('@ROOT@','@HOME@/proj',NULL,1794773547758);
INSERT INTO worktree VALUES('@ROOT@','@S@/opencode/data/opencode/worktree/@ROOT6@/feature one','git',1794773548524);
CREATE TABLE IF NOT EXISTS "migration" (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL);
INSERT INTO migration VALUES('20260127222353_familiar_lady_ursula',1794773547670);
INSERT INTO migration VALUES('20260211171708_add_project_commands',1794773547670);
INSERT INTO migration VALUES('20260213144116_wakeful_the_professor',1794773547670);
INSERT INTO migration VALUES('20260225215848_workspace',1794773547670);
INSERT INTO migration VALUES('20260227213759_add_session_workspace_id',1794773547670);
INSERT INTO migration VALUES('20260228203230_blue_harpoon',1794773547670);
INSERT INTO migration VALUES('20260303231226_add_workspace_fields',1794773547670);
INSERT INTO migration VALUES('20260309230000_move_org_to_state',1794773547670);
INSERT INTO migration VALUES('20260312043431_session_message_cursor',1794773547670);
INSERT INTO migration VALUES('20260323234822_events',1794773547670);
INSERT INTO migration VALUES('20260410174513_workspace-name',1794773547670);
INSERT INTO migration VALUES('20260413175956_chief_energizer',1794773547670);
INSERT INTO migration VALUES('20260423070820_add_icon_url_override',1794773547670);
INSERT INTO migration VALUES('20260427172553_slow_nightmare',1794773547670);
INSERT INTO migration VALUES('20260428004200_add_session_path',1794773547670);
INSERT INTO migration VALUES('20260501142318_next_venus',1794773547670);
INSERT INTO migration VALUES('20260504145000_add_sync_owner',1794773547670);
INSERT INTO migration VALUES('20260507164347_add_workspace_time',1794773547670);
INSERT INTO migration VALUES('20260510033149_session_usage',1794773547670);
INSERT INTO migration VALUES('20260511000411_data_migration_state',1794773547670);
INSERT INTO migration VALUES('20260511173437_session-metadata',1794773547670);
INSERT INTO migration VALUES('20260601010001_normalize_storage_paths',1794773547670);
INSERT INTO migration VALUES('20260601202201_amazing_prowler',1794773547670);
INSERT INTO migration VALUES('20260602002951_lowly_union_jack',1794773547670);
INSERT INTO migration VALUES('20260602182828_add_project_directories',1794773547670);
INSERT INTO migration VALUES('20260603001617_session_message_projection_indexes',1794773547670);
INSERT INTO migration VALUES('20260603040000_session_message_projection_order',1794773547670);
INSERT INTO migration VALUES('20260603141458_session_input_inbox',1794773547670);
INSERT INTO migration VALUES('20260603160727_jittery_ezekiel_stane',1794773547670);
INSERT INTO migration VALUES('20260604172448_event_sourced_session_input',1794773547670);
INSERT INTO migration VALUES('20260605003541_add_session_context_snapshot',1794773547670);
INSERT INTO migration VALUES('20260605042240_add_context_epoch_agent',1794773547670);
INSERT INTO migration VALUES('20260611035744_credential',1794773547670);
INSERT INTO migration VALUES('20260611192811_lush_chimera',1794773547670);
INSERT INTO migration VALUES('20260612174303_project_dir_strategy',1794773547670);
INSERT INTO migration VALUES('20260622142730_simplify_session_context_epoch',1794773547670);
INSERT INTO migration VALUES('20260622170816_reset_v2_session_state',1794773547670);
INSERT INTO migration VALUES('20260622202450_simplify_session_input',1794773547670);
INSERT INTO migration VALUES('20260804233008_loose_psylocke',1794773547670);
INSERT INTO migration VALUES('20260805200742_import_legacy_credentials',1794773547670);
INSERT INTO migration VALUES('20260808023530_workspace_domain',1794773547670);
INSERT INTO migration VALUES('20260811161259_execution_claim_attempts',1794773547670);
INSERT INTO migration VALUES('20260812181746_session_inbox',1794773547670);
INSERT INTO migration VALUES('20260812213948_worktree',1794773547670);
INSERT INTO migration VALUES('20260819222447_session_viewed_state',1794773547670);
INSERT INTO migration VALUES('20260823191254_nullable_workspace_binding',1794773547670);
INSERT INTO migration VALUES('20260910120000_clear_v1_session_permission',1794773547670);
INSERT INTO migration VALUES('20260923013825_project_time_active',1794773547670);
INSERT INTO migration VALUES('20261007190000_azure_cli_external_credential',1794773547670);
INSERT INTO migration VALUES('20261010180456_project_time_archived',1794773547670);
CREATE UNIQUE INDEX `event_aggregate_seq_idx` ON `event` (`aggregate_id`,`seq`);
CREATE INDEX `event_aggregate_type_seq_idx` ON `event` (`aggregate_id`,`type`,`seq`);
CREATE UNIQUE INDEX `permission_project_action_resource_idx` ON `permission` (`project_id`,`action`,`resource`);
CREATE INDEX `session_inbox_session_delivery_seq_idx` ON `session_inbox` (`session_id`,`delivery`,`enqueued_seq`);
CREATE UNIQUE INDEX `session_inbox_session_enqueued_seq_idx` ON `session_inbox` (`session_id`,`enqueued_seq`);
CREATE UNIQUE INDEX `session_message_session_seq_idx` ON `session_message` (`session_id`,`seq`);
CREATE INDEX `session_message_session_type_seq_idx` ON `session_message` (`session_id`,`type`,`seq`);
CREATE INDEX `session_message_session_time_created_id_idx` ON `session_message` (`session_id`,`time_created`,`id`);
CREATE INDEX `session_message_time_created_idx` ON `session_message` (`time_created`);
CREATE INDEX `session_pending_session_delivery_seq_idx` ON `session_pending` (`session_id`,`delivery`,`admitted_seq`);
CREATE UNIQUE INDEX `session_pending_session_compaction_idx` ON `session_pending` (`session_id`) WHERE "session_pending"."type" = 'compaction';
CREATE UNIQUE INDEX `session_pending_session_admitted_seq_idx` ON `session_pending` (`session_id`,`admitted_seq`);
CREATE INDEX `session_v2_project_idx` ON `session_v2` (`project_id`);
CREATE INDEX `session_v2_workspace_idx` ON `session_v2` (`workspace_id`);
CREATE INDEX `session_v2_parent_idx` ON `session_v2` (`parent_id`);
CREATE INDEX `session_v2_time_suspended_idx` ON `session_v2` (`time_suspended`) WHERE "session_v2"."time_suspended" is not null;
COMMIT;
