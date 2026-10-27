use super::crawler;
use chrono::Utc;
use data_structures::{
    config::{Settings, SettingsFriendsLinksJsonMeta},
    metadata::{self, BasePosts},
};
use regex::Regex;
use reqwest::{ClientBuilder as CL, Proxy};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use std::collections::HashMap;
use std::time::Duration;
use tokio::{self, task::JoinSet};
use tools;
use tracing::{error, info, trace, warn};

use url::{ParseError, Url};

async fn get_joinset_result(
    joinset: &mut JoinSet<Vec<BasePosts>>,
    base_url: &Url,
) -> Result<Vec<BasePosts>, Box<dyn std::error::Error>> {
    while let Some(res) = joinset.join_next().await {
        if let Ok(success) = res
            && !success.is_empty()
        {
            info!("{} 解析成功! 共{}条", base_url, success.len());
            return Ok(success);
        }
    }
    Err("css request failed".into())
}

/// 构建请求客户端
pub fn build_client(timeout: u64, retry_attempts: u32) -> ClientWithMiddleware {
    let timeout = Duration::new(timeout, 0);
    let baseclient = CL::new()
        .timeout(timeout)
        .use_rustls_tls()
        .danger_accept_invalid_certs(true);

    let baseclient = match tools::get_env_var("PROXY") {
        Ok(proxy) => {
            info!("use proxy: {}", proxy);
            baseclient.proxy(Proxy::all(proxy).unwrap())
        }
        Err(_) => baseclient,
    };
    let baseclient = baseclient.build().unwrap();

    let mut client_builder = ClientBuilder::new(baseclient);

    // 根据retry_attempts参数动态配置重试中间件
    if retry_attempts > 0 {
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(retry_attempts);
        client_builder =
            client_builder.with(RetryTransientMiddleware::new_with_policy(retry_policy));
        info!("启用重试中间件，最大重试次数: {}", retry_attempts);
    } else {
        info!("禁用重试中间件");
    }

    client_builder.build()
}

/// 检查link页面解析结果的长度
/// 如果字段`author`、`link`、`avatar`缺失，则返回0
/// 否则，检查`author`和`link`的长度：
/// - 是否为0：若有一个为0，返回0
/// - 二者长度是否相等，如果不等，返回0，如果相等返回该长度
fn check_linkpage_res_length(download_res: &HashMap<&str, Vec<String>>) -> usize {
    if !download_res.contains_key("author")
        || !download_res.contains_key("link")
        || !download_res.contains_key("avatar")
    {
        warn!("字段`author`或字段`link`或字段`avatar`缺失，请检查css规则");
        return 0;
    }
    let author_field = download_res.get("author").unwrap();
    let link_field = download_res.get("link").unwrap();
    if author_field.is_empty() || link_field.is_empty() {
        0
    } else if author_field.len() != link_field.len() {
        warn!(
            "字段`author`长度: {}, 字段`link`长度: {},不一致，请检查css规则",
            author_field.len(),
            link_field.len()
        );
        0
    } else {
        author_field.len()
    }
}

/// 检查屏蔽url，匹配返回true（屏蔽），反之为false（不屏蔽）
fn check_block_site(block_sites: &Vec<String>, url: &str, block_site_reverse: bool) -> bool {
    for pattern in block_sites {
        let re = Regex::new(pattern).unwrap();
        if re.is_match(url) {
            return !block_site_reverse;
        }
    }
    block_site_reverse
}

/// Domain entries are literal, not regular expressions, and cover both HTTP and HTTPS.
pub fn is_blog_blocked(settings: &Settings, link: &str) -> bool {
    let host = Url::parse(link)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned));
    let blocked_domain = host.is_some_and(|host| {
        settings.block_blogs.iter().any(|entry| {
            let entry = entry.trim();
            let url = if entry.contains("://") {
                entry.to_string()
            } else {
                format!("https://{entry}")
            };
            Url::parse(&url).ok().is_some_and(|url| {
                url.host_str()
                    .is_some_and(|blocked| host.eq_ignore_ascii_case(blocked))
            })
        })
    });
    blocked_domain || check_block_site(&settings.block_site, link, settings.block_site_reverse)
}

pub fn is_article_blocked(settings: &Settings, link: &str) -> bool {
    is_blog_blocked(settings, link) || check_block_site(&settings.block_article, link, false)
}

pub fn resolve_avatar_url(avatar: &str, base_url: &str) -> String {
    let avatar = avatar.trim();
    if avatar.is_empty() {
        return String::new();
    }
    // Files in Astro's public/ directory are served directly from the site root.
    let path = avatar.strip_prefix("public/")
        .or_else(|| avatar.strip_prefix("/public/"))
        .map(|path| format!("/{path}"))
        .unwrap_or_else(|| avatar.to_string());
    let resolved = Url::parse(&path).or_else(|_| Url::parse(base_url)?.join(&path));
    match resolved {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url.to_string(),
        _ => String::new(),
    }
}

pub async fn start_crawl_postpages(
    base_postpage_url: String,
    settings: &Settings,
    extra_feed_suffix: String,
    css_rules: &tools::Value,
    client: &ClientWithMiddleware,
) -> Result<Vec<metadata::BasePosts>, Box<dyn std::error::Error>> {
    // check block url
    if is_blog_blocked(settings, &base_postpage_url) {
        return Ok(Vec::new());
    };
    let base_url = match Url::parse(&base_postpage_url) {
        Ok(v) => v,
        Err(e) => {
            error!("postpage_url:{} 解析失败:{}", base_postpage_url, e);
            return Ok(Vec::new());
        }
    };
    // An explicitly configured feed is authoritative; do not race it against autodetection.
    if !extra_feed_suffix.trim().is_empty() {
        let feed_url = base_url.join(extra_feed_suffix.trim())?;
        if let Ok(posts) = crawler::crawl_post_page_feed(feed_url.as_str(), &base_url, client).await
            && !posts.is_empty()
        {
            return Ok(posts);
        }
    }
    let css_rules = css_rules.clone();
    let mut joinset = JoinSet::new();

    if css_rules["post_page_rules"].is_mapping() {
        let css_rules = css_rules
            .get("post_page_rules")
            .unwrap()
            .as_mapping()
            .unwrap();
        let css_rules = css_rules.clone();
        let client_: ClientWithMiddleware = client.clone();
        let base_url_ = base_url.clone();

        for feed_suffix in [
            "atom.xml",
            "feed/atom",
            "rss.xml",
            "rss2.xml",
            "feed",
            "index.xml",
        ] {
            let client = client.clone();
            let base_url = base_url.clone();
            let feed_url = base_url.join(feed_suffix)?;
            joinset.spawn(async move {
                match crawler::crawl_post_page_feed(feed_url.as_str(), &base_url, &client).await {
                    Ok(v) => v,
                    Err(e) => {
                        trace!("{}", e);
                        Vec::new()
                    }
                }
            });
        }

        if let Ok(posts) = get_joinset_result(&mut joinset, &base_url).await {
            info!("使用feed规则解析成功:{}", base_url);
            return Ok(posts);
        }
        joinset.spawn(async move {
            // 获取当前时间
            let mut download_postpage_res =
                match crawler::crawl_post_page(&base_postpage_url, &css_rules, &client_).await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("{}", e);
                        return Vec::new();
                    }
                };
            // 字段缺失检查 - 必须有title、link、created三个字段，且长度相等

            // 检查必需字段是否存在
            if !download_postpage_res.contains_key("title") {
                error!("url: {} 解析结果缺失必需字段`title`", base_postpage_url);
                return Vec::new();
            }
            if !download_postpage_res.contains_key("link") {
                error!("url: {} 解析结果缺失必需字段`link`", base_postpage_url);
                return Vec::new();
            }
            if !download_postpage_res.contains_key("created") {
                error!("url: {} 解析结果缺失必需字段`created`", base_postpage_url);
                return Vec::new();
            }

            // 检查三个必需字段的长度是否相等
            let title_len = download_postpage_res.get("title").unwrap().len();
            let link_len = download_postpage_res.get("link").unwrap().len();
            let created_len = download_postpage_res.get("created").unwrap().len();

            if title_len != link_len || title_len != created_len {
                error!(
                    "url: {} 解析结果字段长度不一致: title={}, link={}, created={}",
                    base_postpage_url, title_len, link_len, created_len
                );
                return Vec::new();
            }

            let length = title_len;

            // 如果没有updated字段，使用created字段的时间
            if !download_postpage_res.contains_key("updated") {
                let created_vec = download_postpage_res.get("created").unwrap().clone();
                download_postpage_res.insert("updated", created_vec);
            }

            let mut format_base_posts = vec![];
            for i in 0..length {
                let mut title = download_postpage_res.get("title").unwrap()[i]
                    .trim()
                    .to_string();
                if title.is_empty() {
                    title = String::from("文章标题获取失败")
                };
                let link = download_postpage_res.get("link").unwrap()[i]
                    .trim()
                    .to_string();
                // 处理相对地址
                let link = match Url::parse(&link) {
                    Ok(_) => link,
                    Err(parse_error) => match parse_error {
                        ParseError::RelativeUrlWithoutBase => match base_url_.join(&link) {
                            Ok(completion_url) => completion_url.to_string(),
                            Err(e) => {
                                warn!("无法拼接相对地址：{},error：{}", link, e);
                                continue;
                            }
                        },
                        _ => {
                            warn!("无法处理地址：{}", link);
                            continue;
                        }
                    },
                };
                // created字段已经保证存在且长度正确
                let created = match tools::strftime_to_string_ymd(
                    download_postpage_res.get("created").unwrap()[i].trim(),
                ) {
                    Ok(date) => date,
                    Err(_) => {
                        warn!("无法解析创建时间");
                        continue;
                    }
                };

                // updated字段已经在前面补充过，保证存在且长度正确
                let updated = match tools::strftime_to_string_ymd(
                    download_postpage_res.get("updated").unwrap()[i].trim(),
                ) {
                    Ok(date) => date,
                    Err(_) => {
                        warn!("无法解析更新时间，使用创建时间");
                        created.clone()
                    }
                };
                let rules = download_postpage_res.get("rules").unwrap();
                let base_post =
                    metadata::BasePosts::new(title, created, updated, link, rules.join(","));
                format_base_posts.push(base_post);
            }
            format_base_posts
        });
        if let Ok(posts) = get_joinset_result(&mut joinset, &base_url).await {
            info!("使用css规则解析成功:{}", base_url);
            Ok(posts)
        } else {
            info!("解析失败:{}", base_url);
            Ok(Vec::new())
        }
    } else {
        error!("css_rule 格式错误");
        panic!("css_rule 格式错误");
    }
}

pub async fn start_crawl_linkpages(
    settings: &Settings,
    css_rules: &tools::Value,
    client: &ClientWithMiddleware,
) -> Vec<metadata::Friends> {
    let mut format_base_friends = vec![];
    let start_urls = &settings.link;
    for linkmeta in start_urls {
        // check block url
        let block_sites = &settings.block_site;
        if check_block_site(block_sites, &linkmeta.link, settings.block_site_reverse) {
            continue;
        };
        let download_linkpage_res = match crawler::crawl_link_page(
            &linkmeta.link,
            &linkmeta.theme,
            &css_rules["link_page_rules"],
            client,
        )
        .await
        {
            Ok(v) => v,
            Err(err) => {
                error!("linkpage:{} 解析失败:{}", linkmeta.link, err);
                continue;
            }
        };
        let length = check_linkpage_res_length(&download_linkpage_res);
        for i in 0..length {
            let author = download_linkpage_res.get("author").unwrap()[i]
                .trim()
                .to_string();
            // TODO 链接拼接检查
            let link = download_linkpage_res.get("link").unwrap()[i]
                .trim()
                .to_string();
            // TODO 链接拼接检查
            let _avatar = download_linkpage_res.get("avatar").unwrap();
            let avatar = if i < _avatar.len() {
                download_linkpage_res.get("avatar").unwrap()[i]
                    .trim()
                    .to_string()
            } else {
                // 默认图片
                String::from("https://sdn.geekzu.org/avatar/57d8260dfb55501c37dde588e7c3852c")
            };
            let tm = Utc::now().with_timezone(&crawler::BEIJING_OFFSET.unwrap());
            let created_at = tools::strptime_to_string_ymdhms(tm);
            let base_post = metadata::Friends::new(author, link, avatar, false, created_at);
            format_base_friends.push(base_post);
        }
    }
    format_base_friends
}

pub async fn start_get_friends_links_from_json(
    json_api_or_path: &str,
    client: &ClientWithMiddleware,
) -> Result<SettingsFriendsLinksJsonMeta, Box<dyn std::error::Error>> {
    // 处理GitHub blob链接，转换为raw链接
    let url = if json_api_or_path.contains("github.com") && json_api_or_path.contains("/blob/") {
        json_api_or_path
            .replace("github.com", "raw.githubusercontent.com")
            .replace("/blob/", "/")
    } else {
        json_api_or_path.to_string()
    };

    // 向json_api_or_path发起请求
    let res = client.get(&url).send().await?;
    let content = res.text().await?;

    // 尝试解析为JSON
    match serde_json::from_str(&content) {
        Ok(json) => Ok(json),
        Err(_) => {
            // 如果JSON解析失败，尝试解析为TypeScript/JS格式
            info!("JSON解析失败，尝试作为TypeScript/JS文件解析: {}", url);
            parse_friends_from_ts(&content)
        }
    }
}

fn parse_friends_from_ts(content: &str) -> Result<SettingsFriendsLinksJsonMeta, Box<dyn std::error::Error>> {
    let mut friends = Vec::new();
    let mut lost_friends = Vec::new();
    
    // 匹配对象结构 { key: value, ... }
    // 使用简单的正则匹配，假设对象结构比较标准
    let object_re = Regex::new(r"\{\s*([^{}]+?)\s*\}").unwrap();
    
    // 匹配字段，支持 name/title, link/url/siteurl, avatar/img/icon/imgurl
    // 支持单引号和双引号
    let name_re = Regex::new(r#"\b(?:name|title)\s*:\s*['"](.*?)['"]"#).unwrap();
    let link_re = Regex::new(r#"\b(?:link|url|siteurl)\s*:\s*['"](.*?)['"]"#).unwrap();
    let avatar_re = Regex::new(r#"\b(?:avatar|img|icon|imgurl)\s*:\s*['"](.*?)['"]"#).unwrap();
    let suffix_re = Regex::new(r#"\b(?:suffix|rss)\s*:\s*['"](.*?)['"]"#).unwrap();

    for cap in object_re.captures_iter(content) {
        let obj_content = &cap[1];
        
        let name = name_re.captures(obj_content).map(|c| c[1].to_string());
        let link = link_re.captures(obj_content).map(|c| c[1].to_string());
        let avatar = avatar_re.captures(obj_content).map(|c| c[1].to_string());
        let suffix = suffix_re.captures(obj_content).map(|c| c[1].to_string());
        let lost = obj_content.contains("已失联");
        
        if let (Some(name), Some(link), Some(avatar)) = (name, link, avatar) {
            let mut friend_item = vec![name, link, avatar];
            if let Some(s) = suffix {
                friend_item.push(s);
            }
            if lost {
                lost_friends.push(friend_item);
            } else {
                friends.push(friend_item);
            }
        }
    }
    
    if friends.is_empty() && lost_friends.is_empty() {
        return Err("无法从文件中解析出友链信息".into());
    }

    Ok(SettingsFriendsLinksJsonMeta {
        friends,
        lost_friends,
    })
}

pub async fn start_crawl_detailpages(
    url: &str,
    client: &ClientWithMiddleware,
) -> Result<String, Box<dyn std::error::Error>> {
    let res = client.get(url).send().await?;

    Ok(res.text().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blog_and_article_filters() {
        let mut settings: Settings =
            serde_yaml::from_str(include_str!("../../fc_settings.yaml")).unwrap();
        settings.block_site.clear();
        settings.block_blogs = vec!["example.com".into(), "https://blog.example.org/".into()];
        assert!(is_blog_blocked(&settings, "http://example.com/post/1"));
        assert!(is_blog_blocked(&settings, "https://BLOG.example.org/feed"));
        assert!(!is_blog_blocked(&settings, "https://notexample.com/"));
        assert!(!is_blog_blocked(&settings, "https://example.com.evil.test/"));
        assert!(!is_blog_blocked(&settings, "https://sub.example.com/"));
        assert!(is_article_blocked(&settings, "https://docs.asoe.cn/project/eSearch/"));
        assert!(is_article_blocked(&settings, "https://docs.asoe.cn/friends/"));
        assert!(!is_article_blocked(&settings, "https://docs.asoe.cn/blog/21/"));
        assert!(!is_blog_blocked(&settings, "https://docs.asoe.cn/"));
    }

    #[test]
    fn test_local_avatar_paths_use_the_blog_base_url() {
        let expected = "https://blueke.top/images/friends/081fe612c98ff538.webp";
        for path in ["/images/friends/081fe612c98ff538.webp", "public/images/friends/081fe612c98ff538.webp", "/public/images/friends/081fe612c98ff538.webp"] {
            assert_eq!(resolve_avatar_url(path, "https://blueke.top/"), expected);
        }
        assert_eq!(resolve_avatar_url("https://example.com/avatar.png", "https://blueke.top/"), "https://example.com/avatar.png");
        assert_eq!(resolve_avatar_url("", "https://blueke.top/"), "");
        assert_eq!(resolve_avatar_url("javascript:alert(1)", "https://blueke.top/"), "");
    }

    #[test]
    fn test_old_settings_without_new_filters() {
        let mut yaml: serde_yaml::Value =
            serde_yaml::from_str(include_str!("../../fc_settings.yaml")).unwrap();
        let mapping = yaml.as_mapping_mut().unwrap();
        mapping.remove(&serde_yaml::Value::String("BLOCK_BLOGS".into()));
        mapping.remove(&serde_yaml::Value::String("BLOCK_ARTICLE".into()));
        let settings: Settings = serde_yaml::from_value(yaml).unwrap();
        assert!(settings.block_blogs.is_empty());
        assert!(settings.block_article.is_empty());
    }

    #[test]
    fn test_build_client_with_retry() {
        // 测试启用重试的客户端构建
        let _client = build_client(10, 3);
        // 这里主要测试函数不会panic，实际的重试逻辑需要集成测试
        // ClientWithMiddleware没有公开的inner方法，只测试构建不报错
    }

    #[test]
    fn test_build_client_without_retry() {
        // 测试禁用重试的客户端构建
        let _client = build_client(10, 0);
        // 主要测试函数调用成功
    }

    #[test]
    fn test_retry_attempts_zero() {
        // 测试retry_attempts为0时的行为
        let _client = build_client(5, 0);
        // 确保不会panic
    }

    #[test]
    fn test_retry_attempts_large() {
        // 测试较大的retry_attempts值
        let _client = build_client(5, 10);
        // 确保不会panic
    }

    #[test]
    fn test_parse_friends_from_ts_with_lost_friends() {
        let content = r#"
export const friends = [
  { title: "正常友链", avatar: "https://a.example/avatar.jpg", link: "https://a.example", rss: "https://a.example/rss.xml", tags: ["友链"] },
];
export const lostFriends = [
  { title: "失联友链", avatar: "https://b.example/avatar.jpg", link: "https://b.example", tags: ["已失联"] },
];
"#;
        let parsed = parse_friends_from_ts(content).unwrap();
        assert_eq!(parsed.friends.len(), 1);
        assert_eq!(parsed.friends[0][0], "正常友链");
        assert_eq!(parsed.lost_friends.len(), 1);
        assert_eq!(parsed.lost_friends[0][0], "失联友链");
    }
}
