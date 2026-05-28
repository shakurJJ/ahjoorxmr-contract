#![allow(dead_code)]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, token, Address, Bytes,
    BytesN, Env, Map, String, Symbol, Vec,
};

/// Savings goal with on-chain milestone celebrations
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavingsGoal {
    /// Goal ID
    pub goal_id: u32,
    /// Member address
    pub member: Address,
    /// Group ID
    pub group_id: u32,
    /// Goal name
    pub name: String,
    /// Goal description
    pub description: String,
    /// Target amount
    pub target_amount: i128,
    /// Current amount saved
    pub current_amount: i128,
    /// Token address
    pub token: Address,
    /// Goal creation timestamp
    pub created_at: u64,
    /// Target completion date
    pub target_date: u64,
    /// Goal status
    pub status: GoalStatus,
    /// Priority level
    pub priority: u32,
    /// Category
    pub category: String,
    /// Milestones
    pub milestones: Vec<Milestone>,
    /// Completed milestones
    pub completed_milestones: Vec<u32>,
    /// Metadata
    pub metadata: Map<String, String>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    /// Milestone ID
    pub milestone_id: u32,
    /// Percentage of goal to reach (0-100)
    pub percentage: u32,
    /// Amount to reach
    pub amount: i128,
    /// Milestone name
    pub name: String,
    /// Milestone description
    pub description: String,
    /// Reward type
    pub reward_type: RewardType,
    /// Reward amount/value
    pub reward_value: i128,
    /// Celebration event
    pub celebration_event: String,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RewardType {
    Bonus = 0,
    NFT = 1,
    Badge = 2,
    PointsMultiplier = 3,
    FeesWaived = 4,
    ExtraVotingPower = 5,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoalStatus {
    Active = 0,
    Paused = 1,
    Completed = 2,
    Abandoned = 3,
    Failed = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneCelebration {
    /// Celebration ID
    pub celebration_id: u32,
    /// Goal ID
    pub goal_id: u32,
    /// Milestone ID
    pub milestone_id: u32,
    /// Member address
    pub member: Address,
    /// Celebration timestamp
    pub timestamp: u64,
    /// Celebration type
    pub celebration_type: CelebrationType,
    /// Celebration message
    pub message: String,
    /// Reward issued
    pub reward_issued: bool,
    /// Reward details
    pub reward_details: Map<String, String>,
    /// Witnesses (other members who celebrated)
    pub witnesses: Vec<Address>,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CelebrationType {
    MilestoneReached = 0,
    GoalCompleted = 1,
    PersonalRecord = 2,
    GroupCelebration = 3,
    SpecialAchievement = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalContribution {
    /// Contribution ID
    pub contribution_id: u32,
    /// Goal ID
    pub goal_id: u32,
    /// Member address
    pub member: Address,
    /// Amount contributed
    pub amount: i128,
    /// Contribution timestamp
    pub timestamp: u64,
    /// Source (e.g., "round_payout", "manual_deposit", "bonus")
    pub source: String,
    /// Transaction hash
    pub tx_hash: BytesN<32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalProgress {
    /// Goal ID
    pub goal_id: u32,
    /// Current amount
    pub current_amount: i128,
    /// Target amount
    pub target_amount: i128,
    /// Percentage completed
    pub percentage_completed: u32,
    /// Days remaining
    pub days_remaining: i64,
    /// Estimated completion date
    pub estimated_completion: u64,
    /// Velocity (amount per day)
    pub velocity: i128,
    /// Status
    pub status: GoalStatus,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalAchievementBadge {
    /// Badge ID
    pub badge_id: u32,
    /// Member address
    pub member: Address,
    /// Badge type
    pub badge_type: BadgeType,
    /// Issued timestamp
    pub issued_at: u64,
    /// Badge metadata
    pub metadata: Map<String, String>,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BadgeType {
    GoalCompleted = 0,
    ConsecutiveContributions = 1,
    HighVelocity = 2,
    EarlyCompletion = 3,
    GroupLeader = 4,
    MilestoneChampion = 5,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupGoalSummary {
    /// Group ID
    pub group_id: u32,
    /// Total goals in group
    pub total_goals: u32,
    /// Completed goals
    pub completed_goals: u32,
    /// Active goals
    pub active_goals: u32,
    /// Total amount saved across all goals
    pub total_saved: i128,
    /// Total target amount
    pub total_target: i128,
    /// Average completion percentage
    pub avg_completion_percentage: u32,
    /// Top contributors
    pub top_contributors: Vec<Address>,
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SavingsGoalError {
    GoalNotFound = 1,
    GoalCompleted = 2,
    GoalAbandoned = 3,
    InvalidGoalAmount = 4,
    InvalidMilestone = 5,
    MilestoneNotFound = 6,
    UnauthorizedAccess = 7,
    GoalExpired = 8,
    InvalidContribution = 9,
    CelebrationFailed = 10,
    RewardIssuanceFailed = 11,
    InvalidGoalStatus = 12,
    MilestoneAlreadyCompleted = 13,
}

pub trait SavingsGoalTrackingInterface {
    /// Create a new savings goal
    fn create_goal(
        env: Env,
        member: Address,
        group_id: u32,
        name: String,
        description: String,
        target_amount: i128,
        token: Address,
        target_date: u64,
        priority: u32,
        category: String,
        metadata: Map<String, String>,
    ) -> u32;

    /// Add milestones to a goal
    fn add_milestones(
        env: Env,
        goal_id: u32,
        milestones: Vec<Milestone>,
    );

    /// Contribute to a goal
    fn contribute_to_goal(
        env: Env,
        goal_id: u32,
        member: Address,
        amount: i128,
        source: String,
    ) -> GoalContribution;

    /// Get goal details
    fn get_goal(env: Env, goal_id: u32) -> Option<SavingsGoal>;

    /// Get goal progress
    fn get_goal_progress(env: Env, goal_id: u32) -> GoalProgress;

    /// Check and celebrate milestones
    fn check_and_celebrate_milestones(
        env: Env,
        goal_id: u32,
    ) -> Vec<MilestoneCelebration>;

    /// Celebrate milestone manually
    fn celebrate_milestone(
        env: Env,
        goal_id: u32,
        milestone_id: u32,
        message: String,
    ) -> MilestoneCelebration;

    /// Issue reward for milestone
    fn issue_milestone_reward(
        env: Env,
        celebration_id: u32,
        reward_details: Map<String, String>,
    );

    /// Complete a goal
    fn complete_goal(env: Env, goal_id: u32) -> MilestoneCelebration;

    /// Pause a goal
    fn pause_goal(env: Env, goal_id: u32);

    /// Resume a paused goal
    fn resume_goal(env: Env, goal_id: u32);

    /// Abandon a goal
    fn abandon_goal(env: Env, goal_id: u32);

    /// Get member's goals
    fn get_member_goals(env: Env, member: Address) -> Vec<SavingsGoal>;

    /// Get group goals summary
    fn get_group_goals_summary(env: Env, group_id: u32) -> GroupGoalSummary;

    /// Get goal contributions
    fn get_goal_contributions(env: Env, goal_id: u32) -> Vec<GoalContribution>;

    /// Get milestone celebrations
    fn get_milestone_celebrations(env: Env, goal_id: u32) -> Vec<MilestoneCelebration>;

    /// Issue achievement badge
    fn issue_achievement_badge(
        env: Env,
        member: Address,
        badge_type: BadgeType,
        metadata: Map<String, String>,
    ) -> GoalAchievementBadge;

    /// Get member badges
    fn get_member_badges(env: Env, member: Address) -> Vec<GoalAchievementBadge>;

    /// Get celebration leaderboard
    fn get_celebration_leaderboard(env: Env, group_id: u32) -> Vec<(Address, u32)>;

    /// Update goal metadata
    fn update_goal_metadata(
        env: Env,
        goal_id: u32,
        metadata: Map<String, String>,
    );

    /// Get goals by category
    fn get_goals_by_category(
        env: Env,
        group_id: u32,
        category: String,
    ) -> Vec<SavingsGoal>;

    /// Get top goal contributors
    fn get_top_goal_contributors(
        env: Env,
        group_id: u32,
        limit: u32,
    ) -> Vec<(Address, i128)>;
}
