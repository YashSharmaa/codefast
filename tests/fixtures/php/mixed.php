<html>
<head><title><?= htmlspecialchars($title) ?></title></head>
<body>
<?php foreach ($posts as $post): ?>
  <h2><?php echo $post->title; ?></h2>
  <?php render_post($post, 'full'); ?>
<?php endforeach; ?>
<?php
/** A global-namespace template: functions here are global. */
function render_post($post, string $mode = 'short'): void
{
    $view = new View('post');
    $view->render(['post' => $post]);
}

if (!function_exists('legacy_helper')) {
    function legacy_helper() { return wp_get_option('x'); }
}

class WP_Widget_Custom extends WP_Widget
{
    public function widget($args, $instance)
    {
        add_action('init', [$this, 'setup']);
        $this->form($instance);
        parent::widget($args, $instance);
    }
}
?>
<footer><?php the_footer(); ?></footer>
</body>
</html>
