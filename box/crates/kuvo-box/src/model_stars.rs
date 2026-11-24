use kuvo_engine::{ModelStar, ModelStarSource, StarSave};
use kuvo_server::Caller;
use kuvo_setup::SetupHandle;

pub(crate) struct SetupStars(pub SetupHandle);

fn project(stars: Vec<kuvo_setup::ModelStar>) -> Vec<ModelStar> {
    stars
        .into_iter()
        .map(|star| ModelStar {
            provider: star.provider,
            id: star.id,
        })
        .collect()
}

impl ModelStarSource for SetupStars {
    fn current(&self) -> Option<Vec<ModelStar>> {
        self.0.model_stars().map(project)
    }

    fn save<'a>(&'a self, caller: &'a Caller, model: ModelStar, starred: bool) -> StarSave<'a> {
        Box::pin(async move {
            let original = kuvo_setup::ModelStar {
                provider: model.provider,
                id: model.id,
            };
            Ok(self
                .0
                .change_model_star(caller, original, starred)
                .await?
                .map(project))
        })
    }
}
