use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OllamaModel {
    pub name: String,
    pub is_vision: bool,
}

/// Pick an installed vision model; text-only models cannot analyze photos.
pub async fn best_vision_model() -> Result<Option<String>, String> {
    let models = check_ollama_status().await?;
    Ok(models.iter().find(|m| m.is_vision).map(|m| m.name.clone()))
}

pub async fn check_ollama_status() -> Result<Vec<OllamaModel>, String> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;

    let res = match client.get("http://127.0.0.1:11434/api/tags").send().await {
        Ok(res) => res,
        Err(_) => return Ok(Vec::new()),
    };

    let tags: Value = match res.json().await {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };

    let mut models = Vec::new();

    if let Some(models_arr) = tags.get("models").and_then(|m| m.as_array()) {
        for m in models_arr {
            if let Some(name) = m.get("name").and_then(|n| n.as_str()) {
                let mut is_vision = false;

                // Fallback check based on model name
                let name_lower = name.to_lowercase();
                if name_lower.contains("vision")
                    || name_lower.contains("llava")
                    || name_lower.contains("moondream")
                {
                    is_vision = true;
                }

                // Inspect model capabilities rigorously using /api/show
                if let Ok(show_res) = client
                    .post("http://127.0.0.1:11434/api/show")
                    .json(&json!({"name": name}))
                    .send()
                    .await
                {
                    if let Ok(show_json) = show_res.json::<Value>().await {
                        // Check capabilities array if present
                        if let Some(caps) = show_json.get("capabilities").and_then(|c| c.as_array())
                        {
                            if caps.iter().any(|c| c.as_str().unwrap_or("") == "vision") {
                                is_vision = true;
                            }
                        }

                        // Check details.families for clip/mllama/vision
                        if let Some(families) = show_json
                            .get("details")
                            .and_then(|d| d.get("families"))
                            .and_then(|f| f.as_array())
                        {
                            if families.iter().any(|f| {
                                let fam = f.as_str().unwrap_or("").to_lowercase();
                                fam == "clip" || fam == "vision" || fam == "mllama"
                            }) {
                                is_vision = true;
                            }
                        }
                    }
                }

                models.push(OllamaModel {
                    name: name.to_string(),
                    is_vision,
                });
            }
        }
    }

    Ok(models)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AnalysisResult {
    pub scene_type: String,
    pub people: PeopleAnalysis,
    pub subject: SubjectAnalysis,
    pub composition: CompositionAnalysis,
    pub aesthetic_score: f64,
    #[serde(default)]
    pub blur: BlurAnalysis,
    #[serde(default)]
    pub faces: Vec<FaceDetail>,
    #[serde(default)]
    pub suggestions: Vec<String>,
    pub problems: Vec<String>,
    pub summary: String,
}

impl Default for BlurAnalysis {
    fn default() -> Self {
        BlurAnalysis {
            blurred: false,
            level: 0.0,
            cause: None,
        }
    }
}

impl Default for FaceDetail {
    fn default() -> Self {
        FaceDetail {
            eye_state: "UNCLEAR".into(),
            looking_at_camera: false,
            emotion: None,
            quality: 0.0,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BlurAnalysis {
    pub blurred: bool,
    /// 0.0 (crisp) .. 1.0 (heavily blurred)
    pub level: f64,
    #[serde(default)]
    pub cause: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FaceDetail {
    /// "OPEN" | "PARTIAL" | "CLOSED" | "UNCLEAR"
    pub eye_state: String,
    pub looking_at_camera: bool,
    #[serde(default)]
    pub emotion: Option<String>,
    /// 0.0 .. 1.0
    pub quality: f64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PeopleAnalysis {
    pub count: i32,
    pub all_faces_visible: bool,
    pub eyes_open: bool,
    pub looking_at_camera: bool,
    pub expression_quality: f64,
    pub face_quality: f64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SubjectAnalysis {
    pub clear: bool,
    pub quality: f64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CompositionAnalysis {
    pub score: f64,
    pub issues: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GroupComparisonResult {
    pub primary_keeper: String,
    pub additional_keepers: Vec<String>,
    pub review: Vec<String>,
    pub redundant: Vec<String>,
    pub reasoning: std::collections::HashMap<String, PhotoReasoning>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PhotoReasoning {
    pub decision: String,
    pub compared_to: Option<String>,
    pub reasons: Vec<String>,
}

fn get_analysis_b64(path: &str) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let img = crate::ingest::load_normalized(std::path::Path::new(path), 896)?;
    // Re-encode to JPEG so every incoming format reaches the model identically.
    Ok(STANDARD.encode(crate::ingest::encode_jpeg(&img)?))
}

#[derive(Debug)]
pub enum AnalysisError {
    File(String),
    Service(String),
    Output(String),
}

impl AnalysisError {
    pub fn is_service_failure(&self) -> bool {
        matches!(self, Self::Service(_))
    }
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(message) | Self::Service(message) | Self::Output(message) => {
                f.write_str(message)
            }
        }
    }
}

pub async fn analyze_photo(model_name: &str, image_path: &str) -> Result<AnalysisResult, String> {
    analyze_photo_detailed(model_name, image_path)
        .await
        .map_err(|error| error.to_string())
}

pub async fn analyze_photo_detailed(
    model_name: &str,
    image_path: &str,
) -> Result<AnalysisResult, AnalysisError> {
    // Read and base64 encode image
    let path = image_path.to_string();
    let b64 = tokio::task::spawn_blocking(move || get_analysis_b64(&path))
        .await
        .map_err(|e| AnalysisError::File(e.to_string()))?
        .map_err(AnalysisError::File)?;

    static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("valid local AI client configuration")
    });

    // Create JSON schema for Ollama format option to ensure structured output
    let format = json!({
        "type": "object",
        "properties": {
            "scene_type": { "type": "string" },
            "people": {
                "type": "object",
                "properties": {
                    "count": { "type": "integer" },
                    "all_faces_visible": { "type": "boolean" },
                    "eyes_open": { "type": "boolean" },
                    "looking_at_camera": { "type": "boolean" },
                    "expression_quality": { "type": "number" },
                    "face_quality": { "type": "number" }
                },
                "required": ["count", "all_faces_visible", "eyes_open", "looking_at_camera", "expression_quality", "face_quality"]
            },
            "subject": {
                "type": "object",
                "properties": {
                    "clear": { "type": "boolean" },
                    "quality": { "type": "number" }
                },
                "required": ["clear", "quality"]
            },
            "composition": {
                "type": "object",
                "properties": {
                    "score": { "type": "number" },
                    "issues": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["score", "issues"]
            },
            "blur": {
                "type": "object",
                "properties": {
                    "blurred": { "type": "boolean" },
                    "level": { "type": "number" },
                    "cause": { "type": ["string", "null"] }
                },
                "required": ["blurred", "level"]
            },
            "faces": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "eye_state": { "type": "string", "enum": ["OPEN", "PARTIAL", "CLOSED", "UNCLEAR"] },
                        "looking_at_camera": { "type": "boolean" },
                        "emotion": { "type": ["string", "null"] },
                        "quality": { "type": "number" }
                    },
                    "required": ["eye_state", "looking_at_camera", "quality"]
                }
            },
            "aesthetic_score": { "type": "number" },
            "problems": { "type": "array", "items": { "type": "string" } },
            "suggestions": { "type": "array", "items": { "type": "string" } },
            "summary": { "type": "string" }
        },
        "required": ["scene_type", "people", "subject", "composition", "aesthetic_score", "blur", "faces", "suggestions", "problems", "summary"]
    });

    let payload = json!({
        "model": model_name,
        "prompt": "Inspect this photo and return concise JSON matching the schema. Use short labels, a one-sentence summary, at most three problems and three suggestions. For each clearly visible face report eye state OPEN/PARTIAL/CLOSED/UNCLEAR, looking at camera, emotion and quality. Use UNCLEAR when eyes are too small or obscured; never invent faces or details. Rate blur and subject quality 0-1, composition and aesthetics 0-10. Assess sharpness, exposure and composition objectively.",
        "keep_alive": "15m",
        "stream": false,
        "images": [b64],
        "format": format,
        "options": {
            "temperature": 0.0,
            "num_predict": 1536
        }
    });

    let res = client
        .post("http://127.0.0.1:11434/api/generate")
        .json(&payload)
        .send()
        .await
        .map_err(|e| AnalysisError::Service(e.to_string()))?;

    if !res.status().is_success() {
        return Err(AnalysisError::Service(format!(
            "Ollama API error: {}",
            res.status()
        )));
    }

    let response_json: Value = res
        .json()
        .await
        .map_err(|e| AnalysisError::Service(e.to_string()))?;

    let response_text = response_json
        .get("response")
        .and_then(|r| r.as_str())
        .ok_or_else(|| AnalysisError::Output("No response text found in Ollama output".into()))?;

    let result: AnalysisResult = serde_json::from_str(response_text)
        .map_err(|e| AnalysisError::Output(format!("Failed to parse JSON output: {}", e)))?;

    Ok(result)
}

pub async fn compare_moment_photos(
    model_name: &str,
    photos: &[(String, String)],
) -> Result<GroupComparisonResult, String> {
    // photos is a slice of (photo_id, absolute_path)
    if photos.is_empty() {
        return Err("No photos provided".to_string());
    }

    let mut b64_images = Vec::new();
    let mut mapping_text = String::new();

    for (i, (id, path)) in photos.iter().enumerate() {
        let path = path.clone();
        let b64 = tokio::task::spawn_blocking(move || get_analysis_b64(&path))
            .await
            .map_err(|e| e.to_string())??;
        b64_images.push(b64);
        mapping_text.push_str(&format!("Image {}: {}\n", i + 1, id));
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(300)) // Comparisons take longer
        .build()
        .map_err(|e| e.to_string())?;

    let format = json!({
        "type": "object",
        "properties": {
            "primary_keeper": { "type": "string" },
            "additional_keepers": { "type": "array", "items": { "type": "string" } },
            "review": { "type": "array", "items": { "type": "string" } },
            "redundant": { "type": "array", "items": { "type": "string" } },
            "reasoning": {
               "type": "object",
               "additionalProperties": {
                  "type": "object",
                  "properties": {
                      "decision": { "type": "string", "enum": ["KEEP", "REVIEW", "REMOVE"] },
                      "compared_to": { "type": ["string", "null"] },
                      "reasons": { "type": "array", "items": { "type": "string" } }
                  },
                  "required": ["decision", "reasons"]
               }
            }
        },
        "required": ["primary_keeper", "additional_keepers", "review", "redundant", "reasoning"]
    });

    let prompt = format!(
        "These images represent approximately the same photographic moment.\n\
        Determine which photographs are worth preserving.\n\
        Do NOT automatically assume only one should be kept.\n\
        Preserve multiple photographs when they capture:\n\
        - meaningfully different expressions\n\
        - different people looking good\n\
        - different action\n\
        - emotional/candid moments\n\
        - genuinely different composition\n\
        - information not present in the other images\n\n\
        Reject photographs primarily when they are genuinely redundant or clearly inferior to another image representing the same moment.\n\n\
        Here is the mapping of image sequence to their exact IDs:\n\
        {}\n\
        Use the exact IDs in your JSON output. Do not use 'Image 1', use the ID. If compared_to is not applicable, use null.",
        mapping_text
    );

    let payload = json!({
        "model": model_name,
        "prompt": prompt,
        "stream": false,
        "images": b64_images,
        "format": format,
        "options": {
            "temperature": 0.0
        }
    });

    let res = client
        .post("http://127.0.0.1:11434/api/generate")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Failed to send request to Ollama: {}", e))?;

    if !res.status().is_success() {
        return Err(format!("Ollama API error: {}", res.status()));
    }

    let response_json: Value = res.json().await.map_err(|e| e.to_string())?;

    let response_text = response_json
        .get("response")
        .and_then(|r| r.as_str())
        .ok_or("No response text found in Ollama output")?;

    let result: GroupComparisonResult = serde_json::from_str(response_text)
        .map_err(|e| format!("Failed to parse JSON output: {}\nRaw: {}", e, response_text))?;

    Ok(result)
}
/// Reject incomplete or contradictory responses before they reach the database.
pub fn validate_comparison(
    result: &GroupComparisonResult,
    photos: &[(String, String)],
) -> Result<(), String> {
    let ids: std::collections::HashSet<&str> = photos.iter().map(|(id, _)| id.as_str()).collect();
    let mut decisions = std::collections::HashMap::new();
    for (id, decision) in std::iter::once((&result.primary_keeper, "KEEP"))
        .chain(result.additional_keepers.iter().map(|id| (id, "KEEP")))
        .chain(result.review.iter().map(|id| (id, "REVIEW")))
        .chain(result.redundant.iter().map(|id| (id, "REMOVE")))
    {
        if !ids.contains(id.as_str()) || decisions.insert(id.as_str(), decision).is_some() {
            return Err("AI returned unknown or repeated photo IDs. Compare again.".to_string());
        }
    }
    if decisions.len() != ids.len() || result.reasoning.len() != ids.len() {
        return Err("AI did not evaluate every supplied photo. Compare again.".to_string());
    }
    for (id, reasoning) in &result.reasoning {
        if decisions.get(id.as_str()).copied() != Some(reasoning.decision.as_str()) {
            return Err("AI returned contradictory decisions. Compare again.".to_string());
        }
        if let Some(target) = &reasoning.compared_to {
            if target == id || !ids.contains(target.as_str()) {
                return Err("AI returned an invalid comparison target. Compare again.".to_string());
            }
        }
        if reasoning.decision == "REMOVE"
            && reasoning
                .compared_to
                .as_deref()
                .and_then(|target| decisions.get(target).copied())
                != Some("KEEP")
        {
            return Err(
                "AI removal must reference a photo marked KEEP. Compare again.".to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_requires_complete_consistent_ids_and_a_keeper() {
        let photos = vec![
            ("a".to_string(), String::new()),
            ("b".to_string(), String::new()),
        ];
        let mut result: GroupComparisonResult = serde_json::from_value(json!({
            "primary_keeper": "a", "additional_keepers": [], "review": [], "redundant": ["b"],
            "reasoning": {
                "a": {"decision": "KEEP", "compared_to": null, "reasons": []},
                "b": {"decision": "REMOVE", "compared_to": "a", "reasons": []}
            }
        }))
        .unwrap();
        assert!(validate_comparison(&result, &photos).is_ok());
        result.reasoning.get_mut("b").unwrap().compared_to = Some("unknown".into());
        assert!(validate_comparison(&result, &photos).is_err());
        result.reasoning.get_mut("b").unwrap().compared_to = Some("a".into());
        result.reasoning.get_mut("b").unwrap().decision = "KEEP".into();
        assert!(validate_comparison(&result, &photos).is_err());
        result.reasoning.remove("b");
        assert!(validate_comparison(&result, &photos).is_err());
    }
}

#[cfg(test)]
mod analysis_error_tests {
    use super::*;
    #[test]
    fn only_service_failures_pause_library_analysis() {
        assert!(!AnalysisError::File("bad JPEG".into()).is_service_failure());
        assert!(!AnalysisError::Output("invalid model JSON".into()).is_service_failure());
        assert!(AnalysisError::Service("connection refused".into()).is_service_failure());
    }
}

/// Phase 0 test: `check_ollama_status` always returns `Ok` rather than
/// propagating a connection error, even when Ollama is not running.
/// This ensures the scan and AI analysis never crash on missing Ollama.
#[tokio::test]
async fn ollama_status_never_errors() {
    let result = check_ollama_status().await;
    assert!(
        result.is_ok(),
        "Ollama status check must never produce an error"
    );
}
