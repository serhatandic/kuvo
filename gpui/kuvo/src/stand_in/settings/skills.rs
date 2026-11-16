//! Fixed Skills descriptions for generic Settings rendering tests and shots. All skills logic
//! stays in the real service; these fixtures deliberately do not reproduce it.
use serde_json::{Value, json};

pub fn section() -> Value {
    json!({"id":"skills","title":"Skills","group":"Agents & Tools","icon":"pencil","line":"Instructions and scripts agents load when a task calls for them.","keywords":["skills","instructions"]})
}

pub fn form() -> Value {
    json!({"title":"New Skill","submit":"Create Skill","fields":[
        {"id":"name","title":"Name","kind":"text","value":"","placeholder":"release-notes"},
        {"id":"description","title":"Use it when","kind":"lines","value":"","placeholder":"Writing release notes from the commits since the last tag","line":"The agent reads this to decide when the skill is needed, and loads the instructions only then."},
        {"id":"instructions","title":"Instructions","kind":"lines","mono":true,"value":"","line":"Markdown. Scripts and other files can go in the skill's folder; mention them here so the agent knows to use them."}
    ]})
}

pub fn page() -> Value {
    json!({"id":"skills","title":"Skills","groups":[
        {"id":"available-skills","title":"Skills the Agent Can Use","note":"The agent loads a skill when a task calls for it. Skills in ~/.claude/skills and ~/.agents/skills, and in a project's .opencode/skills, are found on their own.","rows":[
            {"id":"skills","title":"Skills","kind":"list","empty":"None yet.","add":{"id":"add","title":"New Skill…","form":form()},"items":[
                {"id":"skill-release-notes","title":"release-notes","line":"Writing release notes from the commits since the last tag","kind":"switch","value":true,"can":true,"actions":[{"id":"edit","title":"Edit…","form":form()},{"id":"remove","title":"Delete Skill…","style":"destructive","confirm":{"title":"Delete release-notes?","body":"Skills stay in Recently Deleted for 30 days, then are deleted permanently.","button":"Delete"}}]},
                {"id":"skill-code-review","title":"code-review","line":"Reviewing a change for correctness and regressions","kind":"switch","value":true,"can":true}
            ]}
        ]},
        {"id":"other-places","title":"Other Places","note":"A catalog is a web address with an index.json listing its skills.","rows":[]}
    ]})
}
