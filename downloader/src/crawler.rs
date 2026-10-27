use chrono::FixedOffset;
use data_structures::metadata;
use feed_rs::parser;
use html_escape::decode_html_entities;
use reqwest_middleware::ClientWithMiddleware;
use std::{collections::HashMap, vec};
use tracing::warn;
use url::{ParseError, Url};
// time zones
// +08:00
pub static BEIJING_OFFSET: Option<FixedOffset> = FixedOffset::east_opt(8 * 60 * 60);

pub async fn crawl_link_page<'a>(
    url: &str,
    theme: &str,
    css_rule: &serde_yaml::Value,
    client: &ClientWithMiddleware,
) -> Result<HashMap<&'a str, Vec<String>>, Box<dyn std::error::Error>> {
    if css_rule.is_mapping() {
        let theme_rule = match css_rule.get(theme) {
            Some(s) => s,
            None => panic!("`{theme}` field not found in css_rule"),
        };
        let html = client.get(url).send().await?.text().await?;
        let document = nipper::Document::from(&html);
        // 返回结果init
        let mut result: HashMap<&str, Vec<String>> = HashMap::new();
        for rule in ["author", "link", "avatar"] {
            let fields = theme_rule
                .get(rule)
                .ok_or(format!("`{theme}-{rule}` 字段缺失"))?;
            let fields = fields
                .as_sequence()
                .ok_or(format!("`{theme}-{rule}` 字段格式错误"))?;

            let mut res = vec![];
            for field in fields {
                let match_rule: &str = field
                    .get("selector")
                    .ok_or(format!("`{theme}-{rule}-selector` 字段缺失"))?
                    .as_str()
                    .ok_or(format!("`{theme}-{rule}-selector` 字段格式错误"))?;
                let attr = field
                    .get("attr")
                    .ok_or(format!("`{theme}-{rule}-attr` 字段缺失"))?
                    .as_str()
                    .ok_or(format!("`{theme}-{rule}-attr` 字段格式错误"))?;

                for elem in document.select(match_rule).iter() {
                    let parsed_field = match attr {
                        "text" => elem.text().to_string(),
                        _ => match elem.attr(attr).map(|r| r.to_string()) {
                            Some(v) => v,
                            None => continue,
                        },
                    };
                    // 解码 HTML 实体（如 &quot; &amp; 等）
                    let decoded_field = decode_html_entities(&parsed_field).to_string();
                    res.push(decoded_field);
                }
                // 当前规则获取到结果，则认为规则是有效的，短路后续规则
                if !res.is_empty() {
                    break;
                }
            }

            // info!("{:?}",html);

            result.insert(rule, res);
        }
        // DEBUG:
        // if result.len() < 4 {
        //     debug!(
        //         "页面：{}, 使用规则：{:?}, 解析结果：{:#?}",
        //         url, theme, result
        //     );
        // }
        Ok(result)
    } else {
        panic!("css_rule 格式错误");
    }
}

pub async fn crawl_post_page<'a>(
    url: &str,
    css_rules: &serde_yaml::Mapping,
    client: &ClientWithMiddleware,
) -> Result<HashMap<&'a str, Vec<String>>, Box<dyn std::error::Error>> {
    // let html = reqwest::get(url).await?.text().await?;
    // DEBUG:
    // debug!("{}", url);
    let html = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let document = nipper::Document::from(&html);
    // 返回结果init
    let mut result: HashMap<&str, Vec<String>> = HashMap::new();
    // 使用过的css规则
    let mut used_css_rules = vec![];
    'outer: for css_rule in css_rules {
        let use_theme = css_rule
            .0
            .as_str()
            .ok_or("无法解析字段，需要一个字符串".to_string())?;
        used_css_rules.push(use_theme.to_string());
        for current_field in ["title", "link", "created", "updated"] {
            let fields = css_rule
                .1
                .get(current_field)
                .ok_or(format!("`{use_theme}-{current_field}` 字段缺失"))?;
            let fields = fields
                .as_sequence()
                .ok_or(format!("`{use_theme}-{current_field}` 字段格式错误"))?;

            for field in fields {
                let match_rule: &str = field
                    .get("selector")
                    .ok_or(format!("`{use_theme}-{current_field}-selector` 字段缺失"))?
                    .as_str()
                    .ok_or(format!(
                        "`{use_theme}-{current_field}-selector` 字段格式错误"
                    ))?;
                let attr = field
                    .get("attr")
                    .ok_or(format!("`{use_theme}-{current_field}-attr` 字段缺失"))?
                    .as_str()
                    .ok_or(format!("`{use_theme}-{current_field}-attr` 字段格式错误"))?;

                let mut res = vec![];
                for elem in document.select(match_rule).iter() {
                    let parsed_field = match attr {
                        "text" => elem.text().to_string(),
                        _ => match elem.attr(attr).map(|r| r.to_string()) {
                            Some(v) => v,
                            None => continue,
                        },
                    };
                    // 解码 HTML 实体（如 &quot; &amp; 等）
                    let decoded_field = decode_html_entities(&parsed_field).to_string();
                    res.push(decoded_field);
                }
                if !res.is_empty() {
                    // DEBUG:
                    // debug!("{}-{}-{}-{}", use_theme, match_rule, attr, current_field);
                    if !result.contains_key(current_field) {
                        result.insert(current_field, res);
                    }
                    // 全部字段解析完毕
                    if result.len() == 4 {
                        break 'outer;
                    }
                } else {
                    // DEBUG:
                    // debug!(
                    //     "页面：{},字段：{},使用规则:{},解析结果：{:?}",
                    //     url, current_field, use_theme, res
                    // );
                };
            }
        }
    }
    // DEBUG:
    // if result.len() < 4 {
    //     debug!(
    //         "页面：{}, 已使用规则：{:?}, 解析结果：{:?}",
    //         url, used_css_rules, result
    //     );
    // }
    result.insert("rules", used_css_rules);
    Ok(result)
}

pub async fn crawl_post_page_feed(
    url: &str,
    base_url: &Url,
    client: &ClientWithMiddleware,
) -> Result<Vec<metadata::BasePosts>, Box<dyn std::error::Error>> {
    // DEBUG:
    // debug!("feed.....{}", url);
    let html = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    parse_post_feed(html.as_ref(), base_url)
}

fn parse_post_feed(
    html: &[u8],
    base_url: &Url,
) -> Result<Vec<metadata::BasePosts>, Box<dyn std::error::Error>> {
    if let Ok(feed_from_xml) = parser::parse(html) {
        let entries = feed_from_xml.entries;
        // 返回结果init
        let mut format_base_posts = vec![];
        for entry in entries {
            // 标题
            let title = entry.title.map_or(String::from("文章标题获取失败"), |t| {
                // 解码 HTML 实体
                decode_html_entities(&t.content).to_string()
            });
            // url链接
            let link = if !entry.links.is_empty() {
                entry.links[0].href.clone()
            } else {
                warn!("feed无法解析url链接");
                continue;
            };
            // 处理相对地址
            let link = match Url::parse(&link) {
                Ok(_) => link,
                Err(parse_error) => match parse_error {
                    ParseError::RelativeUrlWithoutBase => match base_url.join(&link) {
                        Ok(completion_url) => completion_url.to_string(),
                        Err(e) => {
                            warn!("无法拼接相对地址：{},error:{}", link, e);
                            continue;
                        }
                    },
                    _ => {
                        warn!("无法处理地址：{}", link);
                        continue;
                    }
                },
            };
            // 时间
            let published_time = entry
                .published
                .map(|t| tools::strptime_to_string_ymd(t.fixed_offset()));
            let updated_time = entry
                .updated
                .map(|t| tools::strptime_to_string_ymd(t.fixed_offset()));
            let Some(created) = published_time.clone().or(updated_time.clone()) else {
                // A crawl timestamp is not a publication date and would resurface this page daily.
                warn!("feed entry has no publication/update date, skipping: {}", link);
                continue;
            };
            let updated = updated_time.or(published_time).unwrap_or_else(|| created.clone());
            let base_post =
                metadata::BasePosts::new(title, created, updated, link, "feed".to_string());
            format_base_posts.push(base_post);
        }
        Ok(format_base_posts)
    } else {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_undated_feed_entries_are_not_published_today() {
        let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel>
            <title>Blog</title><link>https://example.com/</link><description>Blog</description>
            <lastBuildDate>Mon, 26 Oct 2026 05:04:22 GMT</lastBuildDate>
            <item><title>Project</title><link>https://example.com/project/</link></item>
            <item><title>Post</title><link>https://example.com/post/</link>
            <pubDate>Sun, 25 Oct 2026 06:32:26 GMT</pubDate></item>
            </channel></rss>"#;
        let posts = parse_post_feed(xml, &Url::parse("https://example.com/").unwrap()).unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].created, "2026-10-25");
    }

    #[test]
    fn test_atom_updated_only_date_is_supported() {
        let xml = br#"<feed xmlns="http://www.w3.org/2005/Atom"><id>https://example.com/</id>
            <title>Blog</title><updated>2026-10-26T05:04:22Z</updated>
            <entry><id>https://example.com/post/</id><title>Post</title>
            <link href="https://example.com/post/"/><updated>2026-10-20T05:04:22Z</updated>
            </entry></feed>"#;
        let posts = parse_post_feed(xml, &Url::parse("https://example.com/").unwrap()).unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].created, "2026-10-20");
    }
}
