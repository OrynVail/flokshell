use std::fmt::Write;

use serde::Deserialize;

// What the shell computed from its settings: colours already resolved, binds
// already resolved to dispatchers. flokd only renders it, once per dialect.
#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Payload {
    pub gaps_in: i64,
    pub gaps_out: i64,
    pub border_size: i64,
    pub active_border: Vec<String>,
    pub active_angle: i64,
    pub inactive_border: Vec<String>,
    pub inactive_angle: i64,
    pub rounding: i64,
    pub blur: Blur,
    pub shadow: Shadow,
    pub layout: String,
    pub workspace_style: String,
    pub binds: Vec<Bind>,
    pub layer_rules: Vec<LayerRule>,
    pub exec_once: Vec<String>,
}

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Blur {
    pub enabled: bool,
    pub size: i64,
    pub passes: i64,
}

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    pub range: i64,
    pub color: String,
}

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Bind {
    pub mods: Vec<String>,
    pub key: String,
    pub dispatcher: String,
    pub arg: String,
    pub flags: String,
}

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct LayerRule {
    pub namespace: String,
    pub no_anim: bool,
    pub blur: bool,
    pub blur_popups: bool,
    pub ignore_alpha: Option<f64>,
}

const HEADER: &str = "Written by flokd from flokshell's settings; edits here are overwritten.";

// Every string that reaches the conf dialect is one line of one field: a
// newline or a comma in the wrong place would be a second directive.
fn clean(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

fn color(s: &str) -> bool {
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'));
    matches!(inner, Some(h) if matches!(h.len(), 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit()))
}

fn colors(list: &[String]) -> Vec<&str> {
    list.iter()
        .map(String::as_str)
        .filter(|c| color(c))
        .collect()
}

fn word(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn namespace(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
}

fn lua_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// A modifier tapped alone (Super_L with SUPER) fires on release, so that
// holding it for a chord does not also trigger it.
fn modifier_alone(b: &Bind) -> bool {
    let k = b.key.to_ascii_lowercase();
    ["super", "alt", "control", "shift"]
        .iter()
        .any(|m| k == format!("{m}_l") || k == format!("{m}_r"))
}

fn mouse(b: &Bind) -> bool {
    b.key.to_ascii_lowercase().starts_with("mouse:")
}

fn lua_flags(b: &Bind) -> Vec<&'static str> {
    let mut out = Vec::new();
    for f in b.flags.chars() {
        let flag = match f.to_ascii_lowercase() {
            'l' => "locked = true",
            'r' => "release = true",
            'e' => "repeating = true",
            'n' => "non_consuming = true",
            _ => continue,
        };
        if !out.contains(&flag) {
            out.push(flag);
        }
    }
    if mouse(b) {
        out.retain(|f| *f == "locked = true");
        out.push("mouse = true");
    } else if modifier_alone(b) && !out.contains(&"release = true") {
        out.push("release = true");
    }
    out
}

fn direction(arg: &str) -> Option<&'static str> {
    match arg.trim() {
        "l" | "left" => Some("left"),
        "r" | "right" => Some("right"),
        "u" | "up" => Some("up"),
        "d" | "down" => Some("down"),
        _ => None,
    }
}

fn delta(arg: &str) -> Option<(i64, i64)> {
    let mut it = arg.split_whitespace();
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    it.next().is_none().then_some((x, y))
}

// The dispatchers the shell's actions resolve to. Anything else is left out
// of the file with a comment: guessing would mean running it as a command.
fn lua_action(b: &Bind) -> Option<String> {
    let arg = b.arg.trim();
    Some(match b.dispatcher.as_str() {
        "" | "exec" => format!("hl.dsp.exec_cmd({})", lua_str(arg)),
        "killactive" => "hl.dsp.window.close()".into(),
        "exit" => "hl.dsp.exit()".into(),
        "togglefloating" => r#"hl.dsp.window.float({ action = "toggle" })"#.into(),
        "pin" => r#"hl.dsp.window.pin({ action = "toggle" })"#.into(),
        "pseudo" => "hl.dsp.window.pseudo()".into(),
        "centerwindow" => "hl.dsp.window.center()".into(),
        "togglegroup" => "hl.dsp.group.toggle()".into(),
        "changegroupactive" => match arg {
            "b" | "prev" => "hl.dsp.group.prev()".into(),
            _ => "hl.dsp.group.next()".into(),
        },
        "fullscreen" => match arg {
            "1" => r#"hl.dsp.window.fullscreen({ mode = "maximized", action = "toggle" })"#.into(),
            "2" => {
                r#"hl.dsp.window.fullscreen_state({ internal = 0, client = 2, action = "toggle" })"#
                    .into()
            }
            _ => "hl.dsp.window.fullscreen()".into(),
        },
        "movefocus" => format!(
            "hl.dsp.focus({{ direction = {} }})",
            lua_str(direction(arg)?)
        ),
        "movewindow" if mouse(b) || arg.is_empty() => "hl.dsp.window.drag()".into(),
        "movewindow" => format!(
            "hl.dsp.window.move({{ direction = {} }})",
            lua_str(direction(arg)?)
        ),
        "resizewindow" => "hl.dsp.window.resize()".into(),
        "resizeactive" => {
            let (x, y) = delta(arg)?;
            format!("hl.dsp.window.resize({{ x = {x}, y = {y}, relative = true }})")
        }
        "cyclenext" => "hl.dsp.window.cycle_next({ next = true })".into(),
        "cycleprev" => "hl.dsp.window.cycle_next({ next = false })".into(),
        "workspace" if !arg.is_empty() => {
            format!("hl.dsp.focus({{ workspace = {} }})", lua_str(arg))
        }
        "movetoworkspace" if !arg.is_empty() => {
            format!("hl.dsp.window.move({{ workspace = {} }})", lua_str(arg))
        }
        "movetoworkspacesilent" if !arg.is_empty() => format!(
            "hl.dsp.window.move({{ workspace = {}, follow = false }})",
            lua_str(arg)
        ),
        "togglespecialworkspace" if arg.is_empty() => "hl.dsp.workspace.toggle_special()".into(),
        "togglespecialworkspace" => format!("hl.dsp.workspace.toggle_special({})", lua_str(arg)),
        "layoutmsg" if !arg.is_empty() => format!("hl.dsp.layout({})", lua_str(arg)),
        "dpms" if matches!(arg, "on" | "off" | "toggle") => {
            format!("hl.dsp.dpms({{ action = {} }})", lua_str(arg))
        }
        _ => return None,
    })
}

fn lua_color(list: &[String], angle: i64) -> Option<String> {
    let cs = colors(list);
    match cs.as_slice() {
        [] => None,
        [one] => Some(lua_str(one)),
        many => Some(format!(
            "{{ colors = {{ {} }}, angle = {angle} }}",
            many.iter()
                .map(|c| lua_str(c))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub fn lua(p: &Payload) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "-- {HEADER}\n");

    let exec: Vec<_> = p
        .exec_once
        .iter()
        .filter(|c| !c.trim().is_empty())
        .collect();
    if !exec.is_empty() {
        o.push_str("hl.on(\"hyprland.start\", function()\n");
        for c in exec {
            let _ = writeln!(o, "    hl.exec_cmd({})", lua_str(c));
        }
        o.push_str("end)\n\n");
    }

    o.push_str("hl.config({\n    general = {\n");
    let _ = writeln!(o, "        gaps_in = {},", p.gaps_in);
    let _ = writeln!(o, "        gaps_out = {},", p.gaps_out);
    let _ = writeln!(o, "        border_size = {},", p.border_size);
    let active = lua_color(&p.active_border, p.active_angle);
    let inactive = lua_color(&p.inactive_border, p.inactive_angle);
    if active.is_some() || inactive.is_some() {
        o.push_str("        col = {\n");
        if let Some(c) = active {
            let _ = writeln!(o, "            active_border = {c},");
        }
        if let Some(c) = inactive {
            let _ = writeln!(o, "            inactive_border = {c},");
        }
        o.push_str("        },\n");
    }
    if word(&p.layout) {
        let _ = writeln!(o, "        layout = {},", lua_str(&p.layout));
    }
    o.push_str("    },\n    decoration = {\n");
    let _ = writeln!(o, "        rounding = {},", p.rounding);
    let _ = writeln!(o, "        shadow = {{");
    let _ = writeln!(o, "            enabled = {},", p.shadow.enabled);
    let _ = writeln!(o, "            range = {},", p.shadow.range);
    if color(&p.shadow.color) {
        let _ = writeln!(o, "            color = {},", lua_str(&p.shadow.color));
    }
    o.push_str("        },\n");
    let _ = writeln!(
        o,
        "        blur = {{ enabled = {}, size = {}, passes = {} }},",
        p.blur.enabled, p.blur.size, p.blur.passes
    );
    o.push_str("    },\n    animations = { enabled = true },\n})\n\n");

    o.push_str(
        "hl.curve(\"flokBezier\", { type = \"bezier\", points = { {0.4, 0.0}, {0.2, 1.0} } })\n",
    );
    o.push_str("hl.animation({ leaf = \"windows\", enabled = true, speed = 2.5, bezier = \"flokBezier\", style = \"popin 80%\" })\n");
    o.push_str("hl.animation({ leaf = \"border\", enabled = true, speed = 2.5, bezier = \"flokBezier\" })\n");
    o.push_str(
        "hl.animation({ leaf = \"fade\", enabled = true, speed = 2.5, bezier = \"flokBezier\" })\n",
    );
    let style = if clean(&p.workspace_style) && !p.workspace_style.is_empty() {
        p.workspace_style.as_str()
    } else {
        "slidefade 20%"
    };
    let _ = writeln!(
        o,
        "hl.animation({{ leaf = \"workspaces\", enabled = true, speed = 2.5, bezier = \"flokBezier\", style = {} }})\n",
        lua_str(style)
    );

    for b in &p.binds {
        if b.key.trim().is_empty() {
            continue;
        }
        let keys = b
            .mods
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(b.key.as_str()))
            .collect::<Vec<_>>()
            .join(" + ");
        let Some(action) = lua_action(b) else {
            let _ = writeln!(
                o,
                "-- skipped {}: {} {}",
                lua_str(&keys),
                lua_str(&b.dispatcher),
                lua_str(&b.arg)
            );
            continue;
        };
        let flags = lua_flags(b);
        if flags.is_empty() {
            let _ = writeln!(o, "hl.bind({}, {action})", lua_str(&keys));
        } else {
            let _ = writeln!(
                o,
                "hl.bind({}, {action}, {{ {} }})",
                lua_str(&keys),
                flags.join(", ")
            );
        }
    }
    o.push('\n');

    for r in &p.layer_rules {
        if !namespace(&r.namespace) {
            continue;
        }
        let mut props = Vec::new();
        if r.no_anim {
            props.push("no_anim = true".to_string());
        }
        if r.blur {
            props.push("blur = true".to_string());
        }
        if r.blur_popups {
            props.push("blur_popups = true".to_string());
        }
        if let Some(a) = r.ignore_alpha.filter(|a| a.is_finite()) {
            props.push(format!("ignore_alpha = {a:.2}"));
        }
        if props.is_empty() {
            continue;
        }
        let _ = writeln!(
            o,
            "hl.layer_rule({{ {}, match = {{ namespace = {} }} }})",
            props.join(", "),
            lua_str(&r.namespace)
        );
    }
    o
}

fn conf_color(list: &[String], angle: i64) -> Option<String> {
    let cs = colors(list);
    match cs.len() {
        0 => None,
        1 => Some(cs[0].to_string()),
        _ => Some(format!("{} {angle}deg", cs.join(" "))),
    }
}

// The legacy grammar names the same dispatchers the shell resolves to, so a
// bind passes through once it is known to be one line.
fn conf_action(b: &Bind) -> Option<String> {
    let arg = b.arg.trim();
    let dispatcher = if b.dispatcher.is_empty() {
        "exec"
    } else {
        &b.dispatcher
    };
    if !word(dispatcher) || !clean(arg) {
        return None;
    }
    if dispatcher == "fullscreen" && arg == "2" {
        return None;
    }
    Some(if arg.is_empty() {
        dispatcher.to_string()
    } else {
        format!("{dispatcher}, {arg}")
    })
}

pub fn conf(p: &Payload) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# {HEADER}\n");
    for c in p
        .exec_once
        .iter()
        .filter(|c| !c.trim().is_empty() && clean(c))
    {
        let _ = writeln!(o, "exec-once = {c}");
    }
    o.push_str("\ngeneral {\n");
    let _ = writeln!(o, "    gaps_in = {}", p.gaps_in);
    let _ = writeln!(o, "    gaps_out = {}", p.gaps_out);
    let _ = writeln!(o, "    border_size = {}", p.border_size);
    if let Some(c) = conf_color(&p.active_border, p.active_angle) {
        let _ = writeln!(o, "    col.active_border = {c}");
    }
    if let Some(c) = conf_color(&p.inactive_border, p.inactive_angle) {
        let _ = writeln!(o, "    col.inactive_border = {c}");
    }
    if word(&p.layout) {
        let _ = writeln!(o, "    layout = {}", p.layout);
    }
    o.push_str("}\n\ndecoration {\n");
    let _ = writeln!(o, "    rounding = {}", p.rounding);
    o.push_str("    shadow {\n");
    let _ = writeln!(o, "        enabled = {}", p.shadow.enabled);
    let _ = writeln!(o, "        range = {}", p.shadow.range);
    if color(&p.shadow.color) {
        let _ = writeln!(o, "        color = {}", p.shadow.color);
    }
    o.push_str("    }\n    blur {\n");
    let _ = writeln!(o, "        enabled = {}", p.blur.enabled);
    let _ = writeln!(o, "        size = {}", p.blur.size);
    let _ = writeln!(o, "        passes = {}", p.blur.passes);
    o.push_str("    }\n}\n\nanimations {\n    enabled = true\n");
    o.push_str("    bezier = flokBezier, 0.4, 0.0, 0.2, 1.0\n");
    o.push_str("    animation = windows, 1, 2.5, flokBezier, popin 80%\n");
    o.push_str("    animation = border, 1, 2.5, flokBezier\n");
    o.push_str("    animation = fade, 1, 2.5, flokBezier\n");
    let style = if clean(&p.workspace_style) && !p.workspace_style.is_empty() {
        p.workspace_style.as_str()
    } else {
        "slidefade 20%"
    };
    let _ = writeln!(
        o,
        "    animation = workspaces, 1, 2.5, flokBezier, {style}\n}}\n"
    );

    for b in &p.binds {
        let key = b.key.trim();
        if key.is_empty() || !clean(key) || key.contains(',') {
            continue;
        }
        if b.mods.iter().any(|m| !word(m)) {
            continue;
        }
        let Some(action) = conf_action(b) else {
            continue;
        };
        let keyword = if mouse(b) {
            "bindm".to_string()
        } else {
            let mut flags: String = b.flags.chars().filter(|c| "lrenm".contains(*c)).collect();
            if modifier_alone(b) && !flags.contains('r') {
                flags.push('r');
            }
            format!("bind{flags}")
        };
        let _ = writeln!(o, "{keyword} = {}, {key}, {action}", b.mods.join(" "));
    }
    o.push('\n');

    for r in &p.layer_rules {
        if !namespace(&r.namespace) {
            continue;
        }
        let mut props = Vec::new();
        if r.no_anim {
            props.push("no_anim on".to_string());
        }
        if r.blur {
            props.push("blur on".to_string());
        }
        if r.blur_popups {
            props.push("blur_popups on".to_string());
        }
        if let Some(a) = r.ignore_alpha.filter(|a| a.is_finite()) {
            props.push(format!("ignore_alpha {a:.2}"));
        }
        if props.is_empty() {
            continue;
        }
        let _ = writeln!(
            o,
            "layerrule = {}, match:namespace {}",
            props.join(", "),
            r.namespace
        );
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bind(mods: &[&str], key: &str, dispatcher: &str, arg: &str, flags: &str) -> Bind {
        Bind {
            mods: mods.iter().map(|s| s.to_string()).collect(),
            key: key.into(),
            dispatcher: dispatcher.into(),
            arg: arg.into(),
            flags: flags.into(),
        }
    }

    fn with_binds(binds: Vec<Bind>) -> Payload {
        Payload {
            binds,
            ..Default::default()
        }
    }

    #[test]
    fn payload_parses_from_the_shell() {
        let p: Payload = serde_json::from_value(json!({
            "gapsIn": 4, "gapsOut": 8, "borderSize": 2,
            "activeBorder": ["rgb(aabbcc)", "rgba(00112233)"], "activeAngle": 45,
            "inactiveBorder": ["rgb(000000)"], "rounding": 12,
            "blur": {"enabled": true, "size": 3, "passes": 2},
            "shadow": {"enabled": false, "range": 10, "color": "rgba(00000080)"},
            "layout": "dwindle", "workspaceStyle": "slidefadevert 20%",
            "binds": [{"mods": ["SUPER"], "key": "D", "dispatcher": "exec", "arg": "flok run dashboard", "flags": ""}],
            "layerRules": [{"namespace": "quickshell", "blur": true, "ignoreAlpha": 0.5}],
            "execOnce": ["flok"]
        }))
        .unwrap();
        assert_eq!(p.gaps_out, 8);
        assert_eq!(p.layer_rules[0].ignore_alpha, Some(0.5));
        assert!(p.blur.enabled);
    }

    #[test]
    fn colors_are_validated_and_gradients_keep_their_angle() {
        assert!(color("rgb(aabbcc)"));
        assert!(color("rgba(aabbcc80)"));
        assert!(!color("rgb(aabbc)"));
        assert!(!color("rgb(aabbcc)\nexec = x"));
        let l = lua_color(&["rgb(aabbcc)".into(), "rgb(ddeeff)".into()], 45).unwrap();
        assert_eq!(
            l,
            r#"{ colors = { "rgb(aabbcc)", "rgb(ddeeff)" }, angle = 45 }"#
        );
        assert_eq!(
            conf_color(&["rgb(aabbcc)".into(), "rgb(ddeeff)".into()], 45).unwrap(),
            "rgb(aabbcc) rgb(ddeeff) 45deg"
        );
        assert_eq!(lua_color(&["junk".into()], 0), None);
    }

    #[test]
    fn lua_binds_carry_flags_and_fixed_semantics() {
        let out = lua(&with_binds(vec![
            bind(
                &[],
                "XF86AudioRaiseVolume",
                "exec",
                "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+",
                "le",
            ),
            bind(&["SUPER"], "Super_L", "exec", "flok run launcher", ""),
            bind(&["SUPER", "SHIFT"], "2", "movetoworkspacesilent", "2", ""),
            bind(&["SUPER", "ALT"], "Right", "resizeactive", "50 0", ""),
            bind(&["SUPER"], "mouse:272", "movewindow", "", ""),
        ]));
        assert!(out.contains(r#"hl.bind("XF86AudioRaiseVolume", hl.dsp.exec_cmd("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+"), { locked = true, repeating = true })"#));
        assert!(out.contains(r#"hl.bind("SUPER + Super_L", hl.dsp.exec_cmd("flok run launcher"), { release = true })"#));
        assert!(out.contains(r#"hl.dsp.window.move({ workspace = "2", follow = false })"#));
        assert!(out.contains(r#"hl.dsp.window.resize({ x = 50, y = 0, relative = true })"#));
        assert!(
            out.contains(r#"hl.bind("SUPER + mouse:272", hl.dsp.window.drag(), { mouse = true })"#)
        );
    }

    #[test]
    fn unknown_dispatchers_are_skipped_not_executed() {
        let out = lua(&with_binds(vec![bind(&["SUPER"], "X", "rm", "-rf ~", "")]));
        assert!(!out.contains("exec_cmd(\"rm"));
        assert!(out.contains("-- skipped"));
        let out = conf(&with_binds(vec![bind(&["SUPER"], "X", "rm -rf", "~", "")]));
        assert!(!out.contains("rm -rf"));
    }

    #[test]
    fn lua_strings_cannot_escape() {
        let out = lua(&with_binds(vec![bind(
            &[],
            "F1",
            "exec",
            "a\") hl.dsp.exit() --",
            "",
        )]));
        assert!(out.contains(r#"hl.dsp.exec_cmd("a\") hl.dsp.exit() --")"#));
    }

    #[test]
    fn conf_lines_cannot_be_split() {
        let out = conf(&with_binds(vec![
            bind(&["SUPER"], "X", "exec", "a\nexec-once = evil", ""),
            bind(&["SUPER"], "Y,Z", "exec", "b", ""),
            bind(&["SUPER\nX"], "Q", "exec", "c", ""),
        ]));
        assert!(!out.contains("evil"));
        assert!(!out.contains("Y,Z"));
        assert!(!out.contains(", c"));
    }

    #[test]
    fn conf_binds_pick_their_keyword() {
        let out = conf(&with_binds(vec![
            bind(
                &[],
                "XF86AudioMute",
                "exec",
                "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
                "l",
            ),
            bind(&["SUPER"], "Super_L", "exec", "flok run launcher", ""),
            bind(&["SUPER"], "mouse:273", "resizewindow", "", ""),
            bind(&["SUPER"], "Q", "killactive", "", ""),
        ]));
        assert!(
            out.contains(
                "bindl = , XF86AudioMute, exec, wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
            )
        );
        assert!(out.contains("bindr = SUPER, Super_L, exec, flok run launcher"));
        assert!(out.contains("bindm = SUPER, mouse:273, resizewindow"));
        assert!(out.contains("bind = SUPER, Q, killactive\n"));
    }

    #[test]
    fn layer_rules_render_in_both_dialects() {
        let p = Payload {
            layer_rules: vec![
                LayerRule {
                    namespace: "quickshell".into(),
                    no_anim: true,
                    blur: true,
                    blur_popups: false,
                    ignore_alpha: Some(0.5),
                },
                LayerRule {
                    namespace: "bad ns".into(),
                    blur: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(lua(&p).contains(
            r#"hl.layer_rule({ no_anim = true, blur = true, ignore_alpha = 0.50, match = { namespace = "quickshell" } })"#
        ));
        assert!(conf(&p).contains(
            "layerrule = no_anim on, blur on, ignore_alpha 0.50, match:namespace quickshell"
        ));
        assert!(!lua(&p).contains("bad ns"));
    }

    #[test]
    fn startup_runs_once() {
        let p = Payload {
            exec_once: vec!["flok".into()],
            ..Default::default()
        };
        assert!(
            lua(&p)
                .contains("hl.on(\"hyprland.start\", function()\n    hl.exec_cmd(\"flok\")\nend)")
        );
        assert!(conf(&p).contains("exec-once = flok"));
    }
}
