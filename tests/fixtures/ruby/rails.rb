# Rails idioms: callbacks, delegation, scopes, concerns, mixins.
module Trackable
  extend ActiveSupport::Concern

  included do
    before_save :track
  end

  module ClassMethods
    def tracked
      where(tracked: true)
    end
  end

  def self.included(base)
    base.extend Helpers
  end

  def track
    self.tracked_at = Time.now
  end
end

class Post < ApplicationRecord
  include Trackable
  extend Searchable
  prepend Audited

  belongs_to :author
  validates :title, presence: true, if: :published?
  validate :title_is_short
  after_commit :notify, unless: [:draft?, :silent?]
  skip_before_action :login
  rescue_from NotFound, with: :not_found

  scope :recent, ->(n) { order(created_at: :desc).limit(n) }
  delegate :name, :email, to: :author
  delegate :size, to: :comments, prefix: true

  def title_is_short
    errors.add(:title) if title.length > 80
  end

  def publish
    send(:notify, 1)
    public_send(:touch)
    method(:notify)
    comments.map(&:body)
    comments.each(&method(:render))
    author&.greet
  end
end
