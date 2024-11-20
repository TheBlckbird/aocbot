use std::fmt::Write;

use chrono::TimeZone;
use matrix_sdk::{
    ruma::{api::client::error::ErrorKind, events::room::message::OriginalRoomMessageEvent},
    Room,
};

use crate::{
    aoc::{client::Parts, day::AocDay, models::PrivateLeaderboardMember},
    context::Context,
    matrix::{
        commands::{parser::ParsedCommand, send_error},
        utils::{error_message, html_message, RoomExt},
    },
    utils::{
        datetime::DateTimeExt,
        fmt::{fmt_timedelta, format_rank},
    },
};

pub async fn invoke(
    event: &OriginalRoomMessageEvent,
    room: Room,
    context: &Context,
    mut cmd: ParsedCommand<'_>,
) -> anyhow::Result<()> {
    let day = match cmd
        .get_from_kwargs_or_args("day")
        .map(|d| d.parse().ok().filter(|d| (1..=25).contains(d)))
        .or_else(|| AocDay::current().map(|d| Some(d.day)))
    {
        Some(Some(d)) => d,
        Some(None) => return send_error(&room, event, "Failed to parse argument 'day'").await,
        None => return send_error(&room, event, "Argument 'day' is required").await,
    };

    let most_recent_year = AocDay::most_recent().year;
    let year = match cmd.get_from_kwargs_or_args("year").map(|y| {
        y.parse()
            .ok()
            .filter(|y| (2015..=most_recent_year).contains(y))
    }) {
        Some(Some(y)) => y,
        Some(None) => return send_error(&room, event, "Failed to parse argument 'year'").await,
        None => most_recent_year,
    };

    let parts = match cmd.get_from_kwargs_or_args("p") {
        Some("1") => Parts::P1,
        Some("2") => Parts::P2,
        Some("both") | None => Parts::Both,
        Some(_) => return send_error(&room, event, "Failed to parse argument 'p'").await,
    };

    let rows = match cmd
        .get_from_kwargs_or_args("rows")
        .map(|y| y.parse().ok().filter(|x| (0..=200).contains(x)))
    {
        Some(Some(x)) => x,
        Some(None) => return send_error(&room, event, "Failed to parse argument 'rows'").await,
        None => context.config.aoc.leaderboard_rows,
    };

    let offset = match cmd
        .get_from_kwargs_or_args("offset")
        .map(|y| y.parse().ok().filter(|x| (0..=200).contains(x)))
    {
        Some(Some(x)) => x,
        Some(None) => return send_error(&room, event, "Failed to parse argument 'offset'").await,
        None => 0,
    };

    let (leaderboard, last_update) = match context
        .aoc_client
        .get_daily_private_leaderboard(year, day, parts)
        .await
    {
        Ok(resp) => resp,
        Err(err) => match err.downcast::<reqwest::Error>() {
            Ok(err) => {
                if let Some(status) = err.status() {
                    room.reply_to(
                        event,
                        error_message(format!(
                            "Failed to fetch private leaderboard for {year} ({status})"
                        )),
                    )
                    .await?;
                    return Ok(());
                } else {
                    return Err(err.into());
                }
            }
            Err(err) => return Err(err),
        },
    };
    let last_update = context
        .config
        .local_timezone
        .from_utc_datetime(&last_update.naive_utc())
        .format_ymd_hms_z();

    let mut members = leaderboard.members.into_values().collect::<Vec<_>>();
    members.sort_unstable();

    let parts_title = match parts {
        Parts::P1 => "/1",
        Parts::P2 => "/2",
        Parts::Both => "",
    };
    let mut leaderboard = format!(
        r#"
<h3>Private Leaderboard (Advent of Code {year}/{day:02}{parts_title})</h3>
<table>
<tr> <th>Rank</th> <th>Local Score</th> <th>Stars</th> <th>Completion</th> <th>AoC Name</th> <th>Matrix User</th> <th>Repository</th> </tr>
"#
    );

    let unlock = AocDay { year, day }.unlock_datetime();

    let mut last_score = u32::MAX;
    let mut rank = 0;
    for (rank, member) in members
        .into_iter()
        .enumerate()
        .map(|(i, member)| {
            if member.local_score != last_score {
                last_score = member.local_score;
                rank = i + 1;
            }
            (rank, member)
        })
        .filter(|(_, m)| m.stars > 0)
        .skip(offset)
        .take(rows)
    {
        let PrivateLeaderboardMember {
            local_score, stars, ..
        } = member;

        let name = member.display_name();

        let matrix_name = context
            .users
            .by_aoc
            .get(&member.id)
            .and_then(|u| u.matrix.as_ref())
            .map(|m| m.matrix_to_uri().to_string())
            .unwrap_or_default();

        let repo = context
            .users
            .by_aoc
            .get(&member.id)
            .and_then(|u| u.repo.as_deref())
            .unwrap_or_default();
        let repo_title = context
            .config
            .aoc
            .repo_rules
            .match_and_replace(repo)
            .map(|m| m.replacement);
        let repo_title = repo_title.as_deref().unwrap_or(repo);

        let (m, m_) = if rank <= 3 {
            ("<b>", "</b>")
        } else {
            Default::default()
        };

        let rank = format_rank(rank);

        let completion = context
            .config
            .local_timezone
            .from_utc_datetime(&member.last_star_ts.naive_utc())
            .format_ymd_hms();

        let start = match parts {
            Parts::P1 | Parts::Both => unlock,
            Parts::P2 => {
                member
                    .completion_day_level
                    .get(&day)
                    .unwrap()
                    .fst
                    .get_star_ts
            }
        };
        let delta = fmt_timedelta(member.last_star_ts - start);

        write!(
            &mut leaderboard,
            r#"
<tr>
    <td>{m}{rank}{m_}</td>
    <td>{m}{local_score}{m_}</td>
    <td>{m}{stars}{m_}</td>
    <td>{completion}({m}{delta}{m_})</td>
    <td>{m}{name}{m_}</td>
    <td>{matrix_name}</td>
    <td>{m}<a href="{repo}">{repo_title}</a>{m_}</td>
</tr>
"#
        )
        .unwrap();
    }

    write!(
        &mut leaderboard,
        r#"
</table>
<sup>Last update: {last_update}</sup>
"#
    )
    .unwrap();

    if let Err(err) = room.reply_to(event, html_message(leaderboard)).await {
        if err
            .as_client_api_error()
            .and_then(|err| err.error_kind())
            .is_some_and(|kind| matches!(kind, ErrorKind::TooLarge))
        {
            room.reply_to(
                event,
                error_message(
                    "The requested leaderboard slice would be too large to fit in a matrix \
                     message. Try to reduce the number of rows.",
                ),
            )
            .await?;
            return Ok(());
        } else {
            return Err(err.into());
        }
    }

    Ok(())
}
