use crate::error::ServerError;
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use std::default::Default;

pub struct ModPortal {
    client: Client,
}

#[derive(Default, Serialize, Deserialize)]
pub enum Sort {
    #[default]
    name,
    created_at,
    updated_at,
}
#[derive(Default, Serialize, Deserialize)]
pub enum SortOrder {
    #[default]
    #[serde(rename = "asc")]
    ascending,
    #[serde(rename = "desc")]
    descending,
}

#[derive(Default, Serialize, Deserialize)]
pub enum Version {
    #[serde(rename = "0.13")]
    version_0_13,
    #[serde(rename = "0.14")]
    version_0_14,
    #[serde(rename = "0.15")]
    version_0_15,
    #[serde(rename = "0.16")]
    version_0_16,
    #[serde(rename = "0.17")]
    version_0_17,
    #[serde(rename = "0.18")]
    version_0_18,
    #[serde(rename = "1.0")]
    version_1_0,
    #[default]
    #[serde(rename = "1.1")]
    version_1_1,
}

pub struct ModListParameter {
    pub hide_deprecated: bool,
    pub page: u32,
    pub page_size: u32,
    pub sort: Sort,
    pub sort_order: SortOrder,
    pub namelist: Vec<String>,
    pub version: Version,
}
impl Default for ModListParameter {
    fn default() -> ModListParameter {
        ModListParameter {
            hide_deprecated: false,
            page: 1,
            page_size: u32::MAX,
            sort: Default::default(),
            sort_order: Default::default(),
            namelist: vec![],
            version: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ModListResponse {
    pagination: Option<Pagination>,
    results: Vec<ModListResult>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Pagination {
    count: u32,
    links: PaginationLinks,
    page: u32,
    page_size: u32,
    page_count: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PaginationLinks {
    first: Option<String>,
    prev: Option<String>,
    next: Option<String>,
    last: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ModResult {
    pub downloads_count: u32,
    pub name: String,
    pub owner: String,
    pub releases: Option<Vec<Release>>,
    pub summary: String,
    pub title: String,
    pub category: Category,
    #[serde(default)]
    pub score: f32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ModListResult {
    pub latest_release: Option<Release>,
    #[serde(flatten)]
    pub result: ModResult,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ShortModResult {
    #[serde(flatten)]
    pub result: ModResult,
    pub thumbnail: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FullModResult {
    #[serde(flatten)]
    pub result: ShortModResult,
    pub changelog: String,
    pub created_at: String, // TODO: ISO8601 timestamp
    pub description: String,
    pub source_url: String,
    pub github_path: String,
    pub homepage: String,
    pub tags: Vec<Tag>,
    pub license: Vec<License>,
    pub deprecated: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Release {
    pub download_url: String,
    pub file_name: String,
    // pub info_json: Object,
    pub released_at: String, // TODO: ISO8601 timestamp
    pub version: String,
    pub sha1: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Tag {
    transportation,
    logistics,
    combat,
    enemies,
    armor,
    environment,
    #[serde(rename = "logistic-network")]
    logistic_network,
    #[serde(rename = "circuit-network")]
    circuit_network,
    storage,
    power,
    manufacturing,
    blueprints,
    cheats,
    mining,
    fluids,
    trains,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct License {
    description: String,
    id: String,
    name: String,
    title: String,
    url: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Category {
    #[serde(rename = "no-category")]
    no_category,
    content,
    overhaul,
    tweaks,
    utilities,
    scenarios,
    #[serde(rename = "mod-packs")]
    mod_packs,
    localizations,
    internal,
}

impl ModPortal {
    pub fn new() -> Result<ModPortal, ServerError> {
        let client = reqwest::ClientBuilder::new().build()?;
        Ok(ModPortal { client })
    }

    // page_size	{an integer or 'max'}
    // sort	{enum, one of name, created_at or updated_at}
    // sort_order	{enum, one of asc or desc}
    // namelist	{array of strings}
    // version	{enum, one of 0.13, 0.14, 0.15, 0.16, 0.17, 0.18, 1.0 or 1.1}
    pub async fn mod_list(&self, parameter: ModListParameter) -> Result<ModListResponse, ServerError> {
        let mut request = self
            .client
            .request(Method::GET, "https://mods.factorio.com/api/mods");
        if !parameter.namelist.is_empty() {
            request = request.query(&[("namelist", parameter.namelist.join(","))]);
        }
        request = request
            .query(&[("hide_deprecated", parameter.hide_deprecated)])
            .query(&[("page", parameter.page)])
            .query(&[("sort", parameter.sort)])
            .query(&[("sort_order", parameter.sort_order)])
            .query(&[("version", parameter.version)])
            ;
        if parameter.page_size == u32::MAX {
            request = request.query(&[("page_size", "max")]);
        } else {
            request = request.query(&[("page_size", parameter.page_size)]);
        }
        let response = request.send().await?.error_for_status()?;
        let response: ModListResponse =  response.json().await?;

        Ok(response)
    }
    
    pub async fn mod_short(&self, mod_name: impl AsRef<str>) -> Result<ShortModResult, ServerError> {
        println!("https://mods.factorio.com/api/mods/{}", mod_name.as_ref());
        Ok(
            self.client.get(
                format!("https://mods.factorio.com/api/mods/{}", mod_name.as_ref())
            )
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        )
    }
    
    pub async fn mod_full(&self, mod_name: impl AsRef<str>) -> Result<FullModResult, ServerError> {
        Ok(
            self.client.get(
                format!("https://mods.factorio.com/api/mods/{}", mod_name.as_ref())
            )
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mod_list() {
        let mod_portal = ModPortal::new().unwrap();
        let parameter = ModListParameter::default();
        let response = mod_portal.mod_list(parameter).await.unwrap();
        println!("{:#?}", response);
    }
}
