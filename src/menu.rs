use colored::*;
use std::io::{self, Write};

pub fn clear_screen() {
    print!("\x1B[2J\x1B[1;1H");
    io::stdout().flush().unwrap();
}

pub fn print_header(text: &str) {
    let top = "╔══════════════════════════════════════════════════════════════════════════╗";
    let mid = format!("║ {:^76} ║", text);
    let bot = "╚══════════════════════════════════════════════════════════════════════════╝";
    println!("\n{}                    {}", top.bright_cyan(), "© 2026 WARDCORE ".bright_black());
    println!("{}                    {}", mid.bright_cyan(), "beta 0.2".bright_black());
    println!("{}", bot.bright_cyan());
    println!();
}

pub fn print_footer() {
    println!("\n{}", format!("{:>78}", "© 2026 WARDCORE").bright_black());
    println!("{}", format!("{:>78}", "beta 0.2").bright_black());
    println!();
}

pub fn print_success(text: &str) {
    println!("{} {}", "✓".green().bold(), text.green());
}

pub fn print_error(text: &str) {
    println!("{} {}", "✗".red().bold(), text.red());
}

pub fn print_info(text: &str) {
    println!("{} {}", "ℹ".blue().bold(), text.bright_white());
}

pub fn print_option(number: usize, text: &str) {
    println!("  {} {}", format!("{}.", number).yellow().bold(), text.white());
}

pub fn get_input(prompt: &str) -> String {
    print!("{} ", prompt.cyan());
    io::stdout().flush().unwrap();

    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().to_string()
}

pub fn get_choice(max: usize) -> usize {
    loop {
        let input = get_input(&format!("Enter your choice (1-{}):", max));

        if let Ok(choice) = input.parse::<usize>() {
            if choice >= 1 && choice <= max {
                return choice;
            }
        }

        print_error("Invalid choice. Please try again.");
    }
}

pub fn wait_for_enter() {
    print!("\n{}", "Press Enter to continue...".bright_black());
    io::stdout().flush().unwrap();
    let mut _input = String::new();
    io::stdin().read_line(&mut _input).unwrap();
}

pub async fn show_main_menu() -> usize {
    clear_screen();
    print_header("ONYX SERVER - MAIN MENU");

    println!("{}", "What would you like to do?".bright_white().bold());
    println!();

    print_option(1, "Create new server");
    print_option(2, "View all servers");
    print_option(3, "Manage server");
    print_option(4, "Quick start guide");
    print_option(5, "Exit");

    println!();
    get_choice(5)
}

pub async fn show_manage_menu() -> usize {
    clear_screen();
    print_header("MANAGE SERVER");

    println!("{}", "Select management option:".bright_white().bold());
    println!();

    print_option(1, "Group settings");
    print_option(2, "Member management");
    print_option(3, "Back to main menu");

    println!();
    get_choice(3)
}

pub async fn show_group_menu() -> usize {
    clear_screen();
    print_header("GROUP SETTINGS");

    println!("{}", "Select option:".bright_white().bold());
    println!();

    print_option(1, "View group info");
    print_option(2, "Setup/rename group");
    print_option(3, "Show invite token");
    print_option(4, "Show public token (channels only)");
    print_option(5, "Convert to channel");
    print_option(6, "Convert to group");
    print_option(7, "Change description");
    print_option(8, "Set max message length");
    print_option(9, "Set media provider");
    print_option(10, "Set max file size");
    print_option(11, "Set allowed file types");
    print_option(12, "Set rate limit (messages per minute)");
    print_option(13, "Back");

    println!();
    get_choice(13)
}

pub async fn show_member_menu() -> usize {
    clear_screen();
    print_header("MEMBER MANAGEMENT");

    println!("{}", "Select option:".bright_white().bold());
    println!();

    print_option(1, "List all members");
    print_option(2, "Add member");
    print_option(3, "Kick member");
    print_option(4, "Ban member");
    print_option(5, "Unban member");
    print_option(6, "Change member role");
    print_option(7, "Back");

    println!();
    get_choice(7)
}

pub fn select_server(servers: &[(String, String, bool, u16, i64, i64)]) -> Option<String> {
    if servers.is_empty() {
        print_error("No servers found!");
        print_info("Please create a server first");
        wait_for_enter();
        return None;
    }

    clear_screen();
    print_header("SELECT SERVER");

    println!("{}", "Available servers:".bright_white().bold());
    println!();

    for (i, (server_name, group_name, is_channel, port, members, _messages)) in servers.iter().enumerate() {
        let type_str = if *is_channel { "Channel" } else { "Group" };
        println!(
            "  {} {} {} {} {} {}",
            format!("{}.", i + 1).yellow().bold(),
            server_name.white().bold(),
            format!("({})", group_name).bright_black(),
            format!("[{}]", type_str).cyan(),
            format!("Port: {}", port).bright_black(),
            format!("{} members", members).bright_black()
        );
    }

    println!();
    print_option(servers.len() + 1, "Back");
    println!();

    let choice = get_choice(servers.len() + 1);

    if choice == servers.len() + 1 {
        None
    } else {
        Some(servers[choice - 1].0.clone())
    }
}
